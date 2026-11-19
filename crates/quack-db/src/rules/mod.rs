//! Rules (identity) and their immutable versions (§3.1).

use quack_core::ids::{AccountId, GuildId, RuleId};
use quack_core::rule::RuleDefinition;
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select, Update};
use crate::pg::{count, int};

/// A rule with its current version's definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The rule.
    pub id: RuleId,
    /// Its server.
    pub guild: GuildId,
    /// The current version.
    pub current_version: u32,
    /// When it was archived, if it is.
    pub archived_at: Option<OffsetDateTime>,
    /// When it was created.
    pub created_at: OffsetDateTime,
    /// Who created it; `None` = Quack.
    pub created_by: Option<AccountId>,
    /// When its identity last changed (edit, archive, restore).
    pub updated_at: OffsetDateTime,
    /// Who changed it last; `None` = Quack.
    pub updated_by: Option<AccountId>,
    /// The current version's definition.
    pub definition: RuleDefinition,
}

/// One immutable version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleVersion {
    /// The rule.
    pub rule: RuleId,
    /// The version number.
    pub version: u32,
    /// The definition.
    pub definition: RuleDefinition,
    /// Partial, rebuilt by the importer (§19.5).
    pub reconstructed: bool,
    /// When it was saved.
    pub created_at: OffsetDateTime,
    /// Who saved it; `None` = Quack.
    pub created_by: Option<AccountId>,
}

/// A version's history entry (without the definition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    /// The version number.
    pub version: u32,
    /// When it was saved.
    pub created_at: OffsetDateTime,
    /// Who saved it.
    pub created_by: Option<AccountId>,
    /// Rebuilt by the importer.
    pub reconstructed: bool,
}

/// What a lock on the rule row saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleLock {
    /// The current version.
    pub current_version: u32,
    /// Whether the rule is archived.
    pub archived: bool,
}

/// A new rule: version 1 of `definition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRule {
    /// A fresh `UUIDv7`.
    pub id: RuleId,
    /// The server.
    pub guild: GuildId,
    /// The validated definition.
    pub definition: RuleDefinition,
    /// Who creates it; `None` = Quack (the starter and honeypot rules).
    pub created_by: Option<AccountId>,
    /// Rebuilt by the importer.
    pub reconstructed: bool,
}

/// Creates a rule and its version 1. An active rule with the same name is
/// [`Conflict::RuleNameTaken`](crate::Conflict::RuleNameTaken).
pub async fn create(conn: &mut PgConnection, rule: &NewRule) -> Result<(), DbError> {
    let by = rule.created_by.map(i64::from);
    obs::run(
        Insert,
        "rules",
        sqlx::query!(
            "INSERT INTO rules (id, guild_id, name, current_version, created_by, updated_by)
             VALUES ($1, $2, $3, 1, $4, $4)",
            rule.id.get(),
            i64::from(rule.guild),
            rule.definition.name,
            by,
        )
        .execute(&mut *conn),
    )
    .await?;
    insert_version(
        conn,
        rule.guild,
        rule.id,
        1,
        &rule.definition,
        rule.created_by,
        rule.reconstructed,
    )
    .await
}

async fn insert_version(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
    version: u32,
    definition: &RuleDefinition,
    by: Option<AccountId>,
    reconstructed: bool,
) -> Result<(), DbError> {
    obs::run(
        Insert,
        "rule_versions",
        sqlx::query!(
            "INSERT INTO rule_versions (rule_id, version, guild_id, definition, reconstructed, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
            rule.get(),
            int::<u32, i32>(version)?,
            i64::from(guild),
            Json(definition) as _,
            reconstructed,
            by.map(i64::from),
        )
        .execute(conn),
    )
    .await?;
    Ok(())
}

/// Locks the rule row `FOR SHARE` (case creation, §4.2 step 10).
pub async fn lock_for_share(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
) -> Result<Option<RuleLock>, DbError> {
    let row = obs::run(
        Select,
        "rules",
        sqlx::query!(
            "SELECT current_version, archived_at FROM rules WHERE guild_id = $1 AND id = $2 FOR SHARE",
            i64::from(guild),
            rule.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(RuleLock {
            current_version: count(r.current_version)?,
            archived: r.archived_at.is_some(),
        })
    })
    .transpose()
}

/// Locks the rule row `FOR UPDATE` (edit, archive, restore).
pub async fn lock_for_update(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
) -> Result<Option<RuleLock>, DbError> {
    let row = obs::run(
        Select,
        "rules",
        sqlx::query!(
            "SELECT current_version, archived_at FROM rules WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            rule.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(RuleLock {
            current_version: count(r.current_version)?,
            archived: r.archived_at.is_some(),
        })
    })
    .transpose()
}

/// Saves `definition` as the next version if the current version is still
/// `expected_version`. Returns the new version, or `None` when the rule
/// changed (`409 rule_changed`) or doesn't exist.
pub async fn add_version(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
    expected_version: u32,
    definition: &RuleDefinition,
    by: Option<AccountId>,
) -> Result<Option<u32>, DbError> {
    let expected = int::<u32, i32>(expected_version)?;
    let updated = obs::run(
        Update,
        "rules",
        sqlx::query_scalar!(
            "UPDATE rules SET current_version = current_version + 1, name = $4, updated_at = now(), updated_by = $5
             WHERE guild_id = $1 AND id = $2 AND current_version = $3
             RETURNING current_version",
            i64::from(guild),
            rule.get(),
            expected,
            definition.name,
            by.map(i64::from),
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    let Some(version) = updated else {
        return Ok(None);
    };
    let version = count(version)?;
    insert_version(conn, guild, rule, version, definition, by, false).await?;
    Ok(Some(version))
}

/// Archives an active rule. Returns whether it was active.
pub async fn archive(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
    by: Option<AccountId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "rules",
        sqlx::query!(
            "UPDATE rules SET archived_at = now(), updated_at = now(), updated_by = $3
             WHERE guild_id = $1 AND id = $2 AND archived_at IS NULL",
            i64::from(guild),
            rule.get(),
            by.map(i64::from),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Restores an archived rule. Returns whether it was archived; an active
/// rule with the same name is [`Conflict::RuleNameTaken`](crate::Conflict::RuleNameTaken).
pub async fn restore(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
    by: Option<AccountId>,
) -> Result<bool, DbError> {
    let done = obs::run(
        Update,
        "rules",
        sqlx::query!(
            "UPDATE rules SET archived_at = NULL, updated_at = now(), updated_by = $3
             WHERE guild_id = $1 AND id = $2 AND archived_at IS NOT NULL",
            i64::from(guild),
            rule.get(),
            by.map(i64::from),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Active rules in the server (checked against the limit under the
/// `guild_settings` lock).
pub async fn count_active(conn: &mut PgConnection, guild: GuildId) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "rules",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM rules WHERE guild_id = $1 AND archived_at IS NULL"#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

mod read;

pub use read::{get, list, load_cache, version, versions};
