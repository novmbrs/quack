//! Reading rules and their versions.

use quack_core::ids::{GuildId, RuleId};
use quack_core::rule::RuleDefinition;
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use super::{Rule, RuleVersion, VersionInfo};
use crate::DbError;
use crate::obs::{self, Select};
use crate::pg::{count, id, int, opt_id};

struct RuleRow {
    id: uuid::Uuid,
    guild_id: i64,
    current_version: i32,
    archived_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    created_by: Option<i64>,
    updated_at: OffsetDateTime,
    updated_by: Option<i64>,
    definition: Json<RuleDefinition>,
}

impl TryFrom<RuleRow> for Rule {
    type Error = DbError;

    fn try_from(r: RuleRow) -> Result<Self, DbError> {
        Ok(Self {
            id: RuleId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            current_version: count(r.current_version)?,
            archived_at: r.archived_at,
            created_at: r.created_at,
            created_by: opt_id(r.created_by)?,
            updated_at: r.updated_at,
            updated_by: opt_id(r.updated_by)?,
            definition: r.definition.0,
        })
    }
}

/// One rule with its current definition.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
) -> Result<Option<Rule>, DbError> {
    let row = obs::run(
        Select,
        "rules",
        sqlx::query_as!(
            RuleRow,
            r#"SELECT r.id, r.guild_id, r.current_version, r.archived_at, r.created_at, r.created_by,
                      r.updated_at, r.updated_by, v.definition AS "definition: Json<RuleDefinition>"
               FROM rules r JOIN rule_versions v ON v.rule_id = r.id AND v.version = r.current_version
               WHERE r.guild_id = $1 AND r.id = $2"#,
            i64::from(guild),
            rule.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Rule::try_from).transpose()
}

/// The server's rules, active or archived, by name.
pub async fn list(
    conn: &mut PgConnection,
    guild: GuildId,
    archived: bool,
) -> Result<Vec<Rule>, DbError> {
    let rows = obs::run(
        Select,
        "rules",
        sqlx::query_as!(
            RuleRow,
            r#"SELECT r.id, r.guild_id, r.current_version, r.archived_at, r.created_at, r.created_by,
                      r.updated_at, r.updated_by, v.definition AS "definition: Json<RuleDefinition>"
               FROM rules r JOIN rule_versions v ON v.rule_id = r.id AND v.version = r.current_version
               WHERE r.guild_id = $1 AND (r.archived_at IS NOT NULL) = $2
               ORDER BY lower(r.name), r.id"#,
            i64::from(guild),
            archived
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Rule::try_from).collect()
}

/// Every active rule of these servers, for the rules cache (§17.6).
pub async fn load_cache(conn: &mut PgConnection, guilds: &[GuildId]) -> Result<Vec<Rule>, DbError> {
    let ids: Vec<i64> = guilds.iter().copied().map(i64::from).collect();
    let rows = obs::run(
        Select,
        "rules",
        sqlx::query_as!(
            RuleRow,
            r#"SELECT r.id, r.guild_id, r.current_version, r.archived_at, r.created_at, r.created_by,
                      r.updated_at, r.updated_by, v.definition AS "definition: Json<RuleDefinition>"
               FROM rules r JOIN rule_versions v ON v.rule_id = r.id AND v.version = r.current_version
               WHERE r.guild_id = ANY($1) AND r.archived_at IS NULL"#,
            &ids
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Rule::try_from).collect()
}

/// One version of a rule in this server.
pub async fn version(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
    version: u32,
) -> Result<Option<RuleVersion>, DbError> {
    let row = obs::run(
        Select,
        "rule_versions",
        sqlx::query!(
            r#"SELECT definition AS "definition: Json<RuleDefinition>", reconstructed, created_at, created_by
               FROM rule_versions WHERE guild_id = $1 AND rule_id = $2 AND version = $3"#,
            i64::from(guild),
            rule.get(),
            int::<u32, i32>(version)?,
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(|r| {
        Ok(RuleVersion {
            rule,
            version,
            definition: r.definition.0,
            reconstructed: r.reconstructed,
            created_at: r.created_at,
            created_by: opt_id(r.created_by)?,
        })
    })
    .transpose()
}

/// A rule's version history, oldest first.
pub async fn versions(
    conn: &mut PgConnection,
    guild: GuildId,
    rule: RuleId,
) -> Result<Vec<VersionInfo>, DbError> {
    let rows = obs::run(
        Select,
        "rule_versions",
        sqlx::query!(
            "SELECT version, created_at, created_by, reconstructed FROM rule_versions
             WHERE guild_id = $1 AND rule_id = $2 ORDER BY version",
            i64::from(guild),
            rule.get(),
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(VersionInfo {
                version: count(r.version)?,
                created_at: r.created_at,
                created_by: opt_id(r.created_by)?,
                reconstructed: r.reconstructed,
            })
        })
        .collect()
}
