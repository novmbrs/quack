//! Servers (`guilds`) and their per-server child rows.

use quack_core::ids::{AccountId, GuildId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{count, id};

/// A Discord server Quack has joined at least once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guild {
    /// The server ID.
    pub id: GuildId,
    /// Its name.
    pub name: String,
    /// Its icon hash, if any.
    pub icon_hash: Option<String>,
    /// Its owner.
    pub owner_id: AccountId,
    /// Whether the server requires 2FA for moderation.
    pub mfa_required: bool,
    /// Whether Quack is currently in it.
    pub installed: bool,
    /// When Quack first joined.
    pub first_joined_at: OffsetDateTime,
    /// When Quack last left, while not installed.
    pub left_at: Option<OffsetDateTime>,
    /// Members as of the last `GUILD_CREATE` and the joins and leaves since
    /// (`None` until Quack has seen it since the count was kept).
    pub member_count: Option<u64>,
}

/// The facts a `GUILD_CREATE` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuildFacts {
    /// The server ID.
    pub id: GuildId,
    /// Its name (1–100 characters).
    pub name: String,
    /// Its icon hash.
    pub icon_hash: Option<String>,
    /// Its owner.
    pub owner_id: AccountId,
    /// Whether moderation requires 2FA.
    pub mfa_required: bool,
}

/// What [`upsert`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upserted {
    /// The server row is new.
    pub new: bool,
    /// Quack had left and is back.
    pub returned: bool,
}

/// Inserts or refreshes a server and marks it installed (§17.5 step 1).
pub async fn upsert(conn: &mut PgConnection, facts: &GuildFacts) -> Result<Upserted, DbError> {
    let row = obs::run(
        Insert,
        "guilds",
        sqlx::query!(
            r#"INSERT INTO guilds (guild_id, name, icon_hash, owner_id, mfa_required)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (guild_id) DO UPDATE SET
                 name = EXCLUDED.name, icon_hash = EXCLUDED.icon_hash, owner_id = EXCLUDED.owner_id,
                 mfa_required = EXCLUDED.mfa_required, installed = true, left_at = NULL, updated_at = now()
               RETURNING old.installed AS "was_installed?""#,
            i64::from(facts.id),
            facts.name,
            facts.icon_hash,
            i64::from(facts.owner_id),
            facts.mfa_required,
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(Upserted {
        new: row.was_installed.is_none(),
        returned: row.was_installed == Some(false),
    })
}

/// Creates every missing per-server row (§17.5 step 2) and reports whether
/// `guild_settings` was created now (a first join, not an imported server).
pub async fn ensure_children(conn: &mut PgConnection, guild: GuildId) -> Result<bool, DbError> {
    let g = i64::from(guild);
    let created = obs::run(
        Insert,
        "guild_settings",
        sqlx::query_scalar!(
            r#"WITH c AS (INSERT INTO case_counters (guild_id) VALUES ($1) ON CONFLICT DO NOTHING),
                    t AS (INSERT INTO ticket_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING),
                    a AS (INSERT INTO activity_log_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING),
                    h AS (INSERT INTO honeypot_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING),
                    s AS (INSERT INTO guild_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING 1)
               SELECT count(*) AS "n!" FROM s"#,
            g
        )
        .fetch_one(conn),
    )
    .await?;
    Ok(created == 1)
}

/// Marks a server as left (§17.5). Returns whether it was installed.
pub async fn mark_left(conn: &mut PgConnection, guild: GuildId) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "guilds",
        sqlx::query!(
            "UPDATE guilds SET installed = false, left_at = now(), updated_at = now()
             WHERE guild_id = $1 AND installed",
            i64::from(guild)
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

struct GuildRow {
    guild_id: i64,
    name: String,
    icon_hash: Option<String>,
    owner_id: i64,
    mfa_required: bool,
    installed: bool,
    first_joined_at: OffsetDateTime,
    left_at: Option<OffsetDateTime>,
    member_count: Option<i32>,
}

impl TryFrom<GuildRow> for Guild {
    type Error = DbError;

    fn try_from(r: GuildRow) -> Result<Self, DbError> {
        Ok(Self {
            id: id(r.guild_id)?,
            name: r.name,
            icon_hash: r.icon_hash,
            owner_id: id(r.owner_id)?,
            mfa_required: r.mfa_required,
            installed: r.installed,
            first_joined_at: r.first_joined_at,
            left_at: r.left_at,
            member_count: r.member_count.map(count).transpose()?,
        })
    }
}

/// One server.
pub async fn get(conn: &mut PgConnection, guild: GuildId) -> Result<Option<Guild>, DbError> {
    let row = obs::run(
        Select,
        "guilds",
        sqlx::query_as!(
            GuildRow,
            "SELECT guild_id, name, icon_hash, owner_id, mfa_required, installed, first_joined_at, left_at, member_count
             FROM guilds WHERE guild_id = $1",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Guild::try_from).transpose()
}

/// The given servers that exist, in any order (route 5 joins them with the
/// account's Discord server list).
pub async fn get_many(conn: &mut PgConnection, guilds: &[GuildId]) -> Result<Vec<Guild>, DbError> {
    let ids: Vec<i64> = guilds.iter().copied().map(i64::from).collect();
    let rows = obs::run(
        Select,
        "guilds",
        sqlx::query_as!(
            GuildRow,
            "SELECT guild_id, name, icon_hash, owner_id, mfa_required, installed, first_joined_at, left_at, member_count
             FROM guilds WHERE guild_id = ANY($1)",
            &ids
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Guild::try_from).collect()
}

/// Stores a server's member count as the gateway read it at `at`
/// (§21.6). A count read before the stored one is ignored, so writes
/// handled out of order keep the newest.
pub async fn set_member_count(
    conn: &mut PgConnection,
    guild: GuildId,
    members: u64,
    at: OffsetDateTime,
) -> Result<(), DbError> {
    let members = i32::try_from(members).unwrap_or(i32::MAX);
    obs::run(
        Update,
        "guilds",
        sqlx::query!(
            "UPDATE guilds SET member_count = $2, member_count_at = $3
             WHERE guild_id = $1 AND (member_count_at IS NULL OR member_count_at <= $3)",
            i64::from(guild),
            members,
            at
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Every server Quack is installed in (startup cache loads).
pub async fn installed(conn: &mut PgConnection) -> Result<Vec<GuildId>, DbError> {
    let rows = obs::run(
        Select,
        "guilds",
        sqlx::query_scalar!("SELECT guild_id FROM guilds WHERE installed ORDER BY guild_id")
            .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(id).collect()
}

/// Takes the server's next case number (§4.2 step 9). Its row lock
/// serializes case creation per server until the transaction ends.
/// Past 2^53 − 1 the counter's check rejects it ([`DbError::Rejected`]).
pub async fn next_case_number(conn: &mut PgConnection, guild: GuildId) -> Result<u64, DbError> {
    let number = obs::run(
        Update,
        "case_counters",
        sqlx::query_scalar!(
            "UPDATE case_counters SET last_number = last_number + 1 WHERE guild_id = $1 RETURNING last_number",
            i64::from(guild)
        )
        .fetch_optional(conn),
    )
    .await?
    .ok_or(DbError::NotFound)?;
    count(number)
}
