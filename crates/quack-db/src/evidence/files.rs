//! The evidence snapshot and its files' copy states (§7.2, §7.3).

use quack_core::evidence::{EvidenceFailure, FileCopyState};
use quack_core::ids::{
    AccountId, AttachmentId, CaseId, ChannelId, EvidenceId, FileId, GuildId, MessageId,
};
use quack_core::limits::NONCE_WINDOW;
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use super::{EmbedSummary, EvidenceFile};
use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{Pg, count, id, int, opt_id};

/// What a capture saw of the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The author.
    pub author: Option<AccountId>,
    /// The text, already cut to 4,000 characters.
    pub content: Option<String>,
    /// When the message was sent.
    pub created_at: OffsetDateTime,
    /// When it was last edited.
    pub edited_at: Option<OffsetDateTime>,
    /// Up to 10 embed summaries.
    pub embeds: Vec<EmbedSummary>,
    /// Files over the caps, only counted.
    pub files_not_saved: u32,
}

/// An admitted file, with its first copy state decided by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFile {
    /// A fresh `UUIDv7` (also the upload nonce).
    pub id: FileId,
    /// Position in the message (1–10).
    pub position: u8,
    /// The original's Discord attachment ID.
    pub attachment_id: Option<AttachmentId>,
    /// The file name.
    pub filename: String,
    /// The content type.
    pub content_type: Option<String>,
    /// The size.
    pub size_bytes: u64,
    /// The original URL.
    pub original_url: String,
    /// `pending`, `link_only`, `too_large`, or `unsupported_type`.
    pub copy_state: FileCopyState,
}

/// Stores the snapshot and its files once (§7.2 step 4): only while the
/// item is `saving` and no snapshot was stored. Returns whether it stored.
pub async fn save_snapshot(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    snapshot: &Snapshot,
    files: &[NewFile],
) -> Result<bool, DbError> {
    let case = obs::run(
        Update,
        "evidence",
        sqlx::query_scalar!(
            "UPDATE evidence SET author_id = $3, content = $4, message_created_at = $5, message_edited_at = $6,
                                 embeds = $7, files_not_saved = $8
             WHERE guild_id = $1 AND id = $2 AND state = 'saving' AND message_created_at IS NULL
             RETURNING case_id",
            i64::from(guild),
            evidence.get(),
            snapshot.author.map(i64::from),
            snapshot.content,
            snapshot.created_at,
            snapshot.edited_at,
            Json(&snapshot.embeds) as _,
            int::<u32, i32>(snapshot.files_not_saved)?,
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    let Some(case) = case else {
        return Ok(false);
    };
    for file in files {
        obs::run(
            Insert,
            "evidence_files",
            sqlx::query!(
                "INSERT INTO evidence_files (id, guild_id, case_id, evidence_id, position, attachment_id, filename,
                                             content_type, size_bytes, original_url, copy_state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                file.id.get(),
                i64::from(guild),
                case,
                evidence.get(),
                i16::from(file.position),
                file.attachment_id.map(i64::from),
                file.filename,
                file.content_type,
                int::<u64, i64>(file.size_bytes)?,
                file.original_url,
                Pg(file.copy_state) as _,
            )
            .execute(&mut *conn),
        )
        .await?;
    }
    Ok(true)
}

struct FileRow {
    id: uuid::Uuid,
    evidence_id: uuid::Uuid,
    case_id: uuid::Uuid,
    position: i16,
    attachment_id: Option<i64>,
    filename: String,
    content_type: Option<String>,
    size_bytes: i64,
    original_url: String,
    copy_state: Pg<FileCopyState>,
    copy_started_at: Option<OffsetDateTime>,
    copy_channel_id: Option<i64>,
    copy_message_id: Option<i64>,
    copy_attachment_id: Option<i64>,
}

impl TryFrom<FileRow> for EvidenceFile {
    type Error = DbError;

    fn try_from(r: FileRow) -> Result<Self, DbError> {
        let copy = match (r.copy_channel_id, r.copy_message_id, r.copy_attachment_id) {
            (Some(c), Some(m), Some(a)) => Some((id(c)?, id(m)?, id(a)?)),
            _ => None,
        };
        Ok(Self {
            id: FileId::from_uuid(r.id),
            evidence: EvidenceId::from_uuid(r.evidence_id),
            case: CaseId::from_uuid(r.case_id),
            position: count(r.position)?,
            attachment_id: opt_id(r.attachment_id)?,
            filename: r.filename,
            content_type: r.content_type,
            size_bytes: count(r.size_bytes)?,
            original_url: r.original_url,
            copy_state: r.copy_state.0,
            copy_started_at: r.copy_started_at,
            copy,
        })
    }
}

macro_rules! query_file {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = FileRow,
            source = "SELECT id, evidence_id, case_id, position, attachment_id, filename, content_type, size_bytes,
                             original_url, copy_state, copy_started_at, copy_channel_id, copy_message_id,
                             copy_attachment_id
                      FROM evidence_files " + $tail,
            args = [$($arg),*]
        )
    };
}

/// Every file of a case, by item and position.
pub async fn files_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Vec<EvidenceFile>, DbError> {
    let rows = obs::run(
        Select,
        "evidence_files",
        query_file!(
            "WHERE guild_id = $1 AND case_id = $2 ORDER BY evidence_id, position",
            i64::from(guild),
            case.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(EvidenceFile::try_from).collect()
}

/// An item's files still to copy.
pub async fn pending_files(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
) -> Result<Vec<EvidenceFile>, DbError> {
    let rows = obs::run(
        Select,
        "evidence_files",
        query_file!(
            "WHERE guild_id = $1 AND evidence_id = $2 AND copy_state = 'pending' ORDER BY position",
            i64::from(guild),
            evidence.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(EvidenceFile::try_from).collect()
}

/// A file of a case in this server, with its source message's location
/// (file routes, §7.4).
pub async fn file(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    file: FileId,
) -> Result<Option<(EvidenceFile, ChannelId, MessageId)>, DbError> {
    let row = obs::run(
        Select,
        "evidence_files",
        sqlx::query!(
            "SELECT e.channel_id, e.message_id FROM evidence_files f JOIN evidence e ON e.id = f.evidence_id
             WHERE f.guild_id = $1 AND f.case_id = $2 AND f.id = $3",
            i64::from(guild),
            case.get(),
            file.get(),
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    let Some(source) = row else {
        return Ok(None);
    };
    let found = obs::run(
        Select,
        "evidence_files",
        query_file!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            file.get()
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(Some((
        EvidenceFile::try_from(found)?,
        id(source.channel_id)?,
        id(source.message_id)?,
    )))
}

/// Starts a pending file's upload window (§10.4), or keeps the current
/// one while it is still open, so a retry inside it reuses the nonce's
/// protection; after [`NONCE_WINDOW`] a new window starts. Only while the
/// item is `saving`. Returns the start, or `None` when the file is no
/// longer pending.
pub async fn start_copy(
    conn: &mut PgConnection,
    guild: GuildId,
    file: FileId,
) -> Result<Option<OffsetDateTime>, DbError> {
    obs::run(
        Update,
        "evidence_files",
        sqlx::query_scalar!(
            r#"UPDATE evidence_files f
               SET copy_started_at = CASE WHEN f.copy_started_at IS NULL
                                            OR f.copy_started_at <= now() - make_interval(secs => $3)
                                          THEN now() ELSE f.copy_started_at END
               FROM evidence e
               WHERE f.guild_id = $1 AND f.id = $2 AND f.copy_state = 'pending'
                 AND e.id = f.evidence_id AND e.state = 'saving'
               RETURNING f.copy_started_at AS "started!""#,
            i64::from(guild),
            file.get(),
            NONCE_WINDOW.as_seconds_f64(),
        )
        .fetch_optional(conn),
    )
    .await
}

/// `pending` → `copied` with the copy's location.
pub async fn mark_copied(
    conn: &mut PgConnection,
    guild: GuildId,
    file: FileId,
    channel: ChannelId,
    message: MessageId,
    attachment: AttachmentId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "evidence_files",
        sqlx::query!(
            "UPDATE evidence_files SET copy_state = 'copied', copy_channel_id = $2, copy_message_id = $3,
                                       copy_attachment_id = $4
             WHERE id = $1 AND guild_id = $5 AND copy_state = 'pending'",
            file.get(),
            i64::from(channel),
            i64::from(message),
            i64::from(attachment),
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `pending` → a state without a copy (`copy_failed`, …).
pub async fn set_file_state(
    conn: &mut PgConnection,
    guild: GuildId,
    file: FileId,
    state: FileCopyState,
) -> Result<bool, DbError> {
    if state == FileCopyState::Copied {
        return Err(DbError::Invalid("use mark_copied".to_owned()));
    }
    let done = obs::run(
        Update,
        "evidence_files",
        sqlx::query!(
            "UPDATE evidence_files SET copy_state = $2
             WHERE id = $1 AND guild_id = $3 AND copy_state = 'pending'",
            file.get(),
            Pg(state) as _,
            i64::from(guild),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Every pending file of the item → `copy_failed`; `not_staff_only` also
/// records why on the item while it is `saving`. Returns how many files
/// changed.
pub async fn fail_pending_files(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    not_staff_only: bool,
) -> Result<u64, DbError> {
    let done = obs::run(
        Update,
        "evidence_files",
        sqlx::query!(
            "UPDATE evidence_files SET copy_state = 'copy_failed'
             WHERE guild_id = $1 AND evidence_id = $2 AND copy_state = 'pending'",
            i64::from(guild),
            evidence.get(),
        )
        .execute(&mut *conn),
    )
    .await?;
    if not_staff_only {
        obs::run(
            Update,
            "evidence",
            sqlx::query!(
                "UPDATE evidence SET copy_skipped_not_staff_only = true
                 WHERE guild_id = $1 AND id = $2 AND state = 'saving'",
                i64::from(guild),
                evidence.get()
            )
            .execute(conn),
        )
        .await?;
    }
    Ok(done.rows_affected())
}

/// `saving` → `saved`, once the snapshot is stored and no file is pending.
pub async fn mark_saved(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "evidence",
        sqlx::query!(
            "UPDATE evidence SET state = 'saved', saved_at = now()
             WHERE guild_id = $1 AND id = $2 AND state = 'saving' AND message_created_at IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM evidence_files WHERE evidence_id = $2 AND copy_state = 'pending')",
            i64::from(guild),
            evidence.get(),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `saving` → `failed`.
pub async fn mark_failed(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    failure: EvidenceFailure,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "evidence",
        sqlx::query!(
            "UPDATE evidence SET state = 'failed', failure = $3 WHERE guild_id = $1 AND id = $2 AND state = 'saving'",
            i64::from(guild),
            evidence.get(),
            Pg(failure) as _,
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}
