//! Writes of the sync job and of settings changes that move tracked
//! messages (§10.2–§10.3). Each is a compare-and-set on the generation the
//! caller read, so a sync that lost a race never overwrites newer state.

use quack_core::ids::{AttachmentId, ChannelId, GuildId, MessageId, MessageRowId};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use super::{Location, Owner, rev};
use crate::jobs::{self, NewJob};
use crate::obs::{self, Update};
use crate::{DbError, Tx};

/// Marks the start of a create (§10.4 step 1): sets `create_started_at`
/// when none is running, or always when `restart` (the window ended and
/// the create is made again). Returns the start, or `None` when the
/// generation changed.
pub async fn start_create(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
    restart: bool,
) -> Result<Option<OffsetDateTime>, DbError> {
    obs::run(
        Update,
        "messages",
        sqlx::query_scalar!(
            r#"UPDATE messages SET create_started_at = CASE WHEN $3 THEN now() ELSE COALESCE(create_started_at, now()) END,
                                   updated_at = now()
               WHERE id = $1 AND generation = $2 AND guild_id = $4 AND state = 'active'
               RETURNING create_started_at AS "started!""#,
            row.get(),
            rev(generation)?,
            restart,
            i64::from(guild),
        )
        .fetch_optional(conn),
    )
    .await
}

/// Records a successful write (§10.3 step 4): `posted_revision` up to
/// `revision`, the created message ID if any, the create window closed.
/// `false` means the message moved meanwhile; the caller then pushes a
/// message it created onto `stale`.
pub async fn record_posted(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
    revision: u32,
    created: Option<MessageId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET posted_revision = GREATEST(posted_revision, $3),
                                 message_id = COALESCE(message_id, $4), create_started_at = NULL,
                                 last_error = NULL, updated_at = now()
             WHERE id = $1 AND generation = $2 AND guild_id = $5 AND state = 'active'",
            row.get(),
            rev(generation)?,
            rev(revision)?,
            created.map(i64::from),
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records the file the message carries (the ticket transcript).
pub async fn set_attachment(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
    attachment: AttachmentId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET attachment_id = $3, updated_at = now()
             WHERE id = $1 AND generation = $2 AND guild_id = $4 AND state = 'active'",
            row.get(),
            rev(generation)?,
            i64::from(attachment),
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Appends a location to clean up (deduplicated; the oldest of more than 5
/// is dropped). Returns whether it was added.
pub async fn push_stale(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    location: Location,
) -> Result<bool, DbError> {
    let entry = Json(vec![location]);
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET stale = (
                 SELECT COALESCE(jsonb_agg(e ORDER BY n), '[]'::jsonb)
                 FROM jsonb_array_elements(stale || $2::jsonb) WITH ORDINALITY AS t(e, n)
                 WHERE n > jsonb_array_length(stale || $2::jsonb) - 5),
               updated_at = now()
             WHERE id = $1 AND guild_id = $3 AND NOT stale @> $2::jsonb",
            row.get(),
            entry as _,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Removes a cleaned-up location. Returns whether it was listed.
pub async fn remove_stale(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    location: Location,
) -> Result<bool, DbError> {
    let entry = Json(location);
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET stale = (
                 SELECT COALESCE(jsonb_agg(e ORDER BY n), '[]'::jsonb)
                 FROM jsonb_array_elements(stale) WITH ORDINALITY AS t(e, n) WHERE e <> $2::jsonb),
               updated_at = now()
             WHERE id = $1 AND guild_id = $3 AND stale @> jsonb_build_array($2::jsonb)",
            row.get(),
            entry as _,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `active` → `gone` at `generation`.
pub async fn mark_gone(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET state = 'gone', create_started_at = NULL, updated_at = now()
             WHERE id = $1 AND generation = $2 AND guild_id = $3 AND state = 'active'",
            row.get(),
            rev(generation)?,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Records a failed sync's machine code (at most 200 characters kept).
pub async fn set_error(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
    code: &str,
) -> Result<bool, DbError> {
    let code: String = code.chars().take(200).collect();
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET last_error = $3, updated_at = now()
             WHERE id = $1 AND generation = $2 AND guild_id = $4",
            row.get(),
            rev(generation)?,
            code,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A public receipt whose create was refused becomes private while its
/// token is valid (§10.3). Returns whether it changed.
pub async fn make_private(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET private = true, create_started_at = NULL, updated_at = now()
             WHERE id = $1 AND generation = $2 AND guild_id = $3 AND state = 'active'
               AND kind = 'case_receipt' AND NOT private AND message_id IS NULL AND token_expires_at > now()",
            row.get(),
            rev(generation)?,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Forgets a receipt's interaction token, only while it is still `token`
/// (a newer token is never cleared).
pub async fn clear_token(
    conn: &mut PgConnection,
    guild: GuildId,
    row: MessageRowId,
    token: &str,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET interaction_token = NULL, token_expires_at = NULL, updated_at = now()
             WHERE id = $1 AND guild_id = $2 AND interaction_token = $3",
            row.get(),
            i64::from(guild),
            token,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Posts the message anew in `channel` (a move, a repost after deletion):
/// the old location goes to `stale`, `generation + 1`, `active`, bumped and
/// synced. Returns whether the row was still at `generation`.
pub async fn relocate(
    tx: &mut Tx,
    guild: GuildId,
    row: MessageRowId,
    generation: u32,
    channel: Option<ChannelId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "messages",
        sqlx::query!(
            "UPDATE messages SET
               stale = CASE WHEN message_id IS NULL OR channel_id IS NULL THEN stale ELSE (
                 SELECT COALESCE(jsonb_agg(e ORDER BY n), '[]'::jsonb)
                 FROM jsonb_array_elements(stale || jsonb_build_array(jsonb_build_object(
                        'channel_id', channel_id::text, 'message_id', message_id::text)))
                      WITH ORDINALITY AS t(e, n)
                 WHERE n > jsonb_array_length(stale) + 1 - 5) END,
               channel_id = $3, message_id = NULL, attachment_id = NULL, generation = generation + 1, create_started_at = NULL,
               state = 'active', wanted_revision = wanted_revision + 1, wanted_at = now(), updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND generation = $4",
            i64::from(guild),
            row.get(),
            channel.map(i64::from),
            rev(generation)?,
        )
        .execute(&mut **tx),
    )
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    jobs::enqueue(tx, &NewJob::sync_message(guild, row)).await?;
    Ok(true)
}

/// Reactivates a `gone` one-per-server message in `channel` (re-enabling
/// a module, §10.2). Returns the row if one was gone.
pub async fn reactivate(
    tx: &mut Tx,
    guild: GuildId,
    owner: Owner,
    channel: ChannelId,
) -> Result<Option<MessageRowId>, DbError> {
    let Owner::Server(kind) = owner else {
        return Err(DbError::Invalid(
            "only one-per-server messages are reactivated".to_owned(),
        ));
    };
    let row = obs::run(
        Update,
        "messages",
        sqlx::query_scalar!(
            "UPDATE messages SET state = 'active', channel_id = $3, message_id = NULL, attachment_id = NULL, generation = generation + 1,
                                 create_started_at = NULL, wanted_revision = wanted_revision + 1, wanted_at = now(),
                                 updated_at = now()
             WHERE guild_id = $1 AND kind = $2 AND state = 'gone' AND case_id IS NULL AND appeal_id IS NULL
               AND ticket_id IS NULL
             RETURNING id",
            i64::from(guild),
            crate::pg::Pg(kind) as _,
            i64::from(channel),
        )
        .fetch_optional(&mut **tx),
    )
    .await?;
    let Some(row) = row.map(MessageRowId::from_uuid) else {
        return Ok(None);
    };
    jobs::enqueue(tx, &NewJob::sync_message(guild, row)).await?;
    Ok(Some(row))
}

/// Setting an appeal channel (§8.3): the posts of open appeals that have no
/// channel or are `gone` move to `channel` and are bumped (one statement,
/// one batch of jobs).
pub async fn bind_waiting_appeal_posts(
    tx: &mut Tx,
    guild: GuildId,
    channel: ChannelId,
) -> Result<Vec<MessageRowId>, DbError> {
    let rows = obs::run(
        Update,
        "messages",
        sqlx::query_scalar!(
            "UPDATE messages m SET channel_id = $2, message_id = NULL, attachment_id = NULL, generation = generation + 1, state = 'active',
                                   create_started_at = NULL, wanted_revision = wanted_revision + 1,
                                   wanted_at = now(), updated_at = now()
             FROM appeals a
             WHERE m.guild_id = $1 AND m.kind = 'appeal_post' AND (m.channel_id IS NULL OR m.state = 'gone')
               AND a.id = m.appeal_id AND a.state IN ('pending', 'needs_information')
             RETURNING m.id",
            i64::from(guild),
            i64::from(channel),
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    let rows: Vec<MessageRowId> = rows.into_iter().map(MessageRowId::from_uuid).collect();
    sync_all(tx, guild, &rows).await?;
    Ok(rows)
}

/// Changing the appeal channel (§8.3): the active posts of open appeals in
/// another channel move to `channel`; their old messages become stale (one
/// statement, one batch of jobs).
pub async fn move_open_appeal_posts(
    tx: &mut Tx,
    guild: GuildId,
    channel: ChannelId,
) -> Result<Vec<MessageRowId>, DbError> {
    let rows = obs::run(
        Update,
        "messages",
        sqlx::query_scalar!(
            "UPDATE messages m SET
               stale = CASE WHEN m.message_id IS NULL OR m.channel_id IS NULL THEN m.stale ELSE (
                 SELECT COALESCE(jsonb_agg(e ORDER BY n), '[]'::jsonb)
                 FROM jsonb_array_elements(m.stale || jsonb_build_array(jsonb_build_object(
                        'channel_id', m.channel_id::text, 'message_id', m.message_id::text)))
                      WITH ORDINALITY AS t(e, n)
                 WHERE n > jsonb_array_length(m.stale) + 1 - 5) END,
               channel_id = $2, message_id = NULL, attachment_id = NULL, generation = m.generation + 1, create_started_at = NULL,
               state = 'active', wanted_revision = m.wanted_revision + 1, wanted_at = now(), updated_at = now()
             FROM appeals a
             WHERE m.guild_id = $1 AND m.kind = 'appeal_post' AND m.state = 'active'
               AND m.channel_id IS DISTINCT FROM $2
               AND a.id = m.appeal_id AND a.state IN ('pending', 'needs_information')
             RETURNING m.id",
            i64::from(guild),
            i64::from(channel),
        )
        .fetch_all(&mut **tx),
    )
    .await?;
    let rows: Vec<MessageRowId> = rows.into_iter().map(MessageRowId::from_uuid).collect();
    sync_all(tx, guild, &rows).await?;
    Ok(rows)
}

async fn sync_all(tx: &mut Tx, guild: GuildId, rows: &[MessageRowId]) -> Result<(), DbError> {
    let jobs: Vec<NewJob> = rows
        .iter()
        .map(|row| NewJob::sync_message(guild, *row))
        .collect();
    jobs::enqueue_many(tx, &jobs).await
}
