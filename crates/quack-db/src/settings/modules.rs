//! Turning optional modules on and off.

use quack_core::ids::{AccountId, GuildId};
use quack_core::modules::ModuleName;
use sqlx::PgConnection;

use crate::DbError;
use crate::obs::{self, Update};
use crate::pg::count;

/// Leaving a server (§17.5): every enabled module is turned off with
/// `disabled_reason = quack_left`. Returns the modules turned off.
pub async fn disable_modules_on_leave(
    conn: &mut PgConnection,
    guild: GuildId,
) -> Result<Vec<ModuleName>, DbError> {
    let row = obs::run(
        Update,
        "ticket_settings",
        sqlx::query!(
            r#"WITH t AS (UPDATE ticket_settings SET enabled = false, disabled_reason = 'quack_left',
                                                   version = version + 1, updated_at = now(), updated_by = NULL
                          WHERE guild_id = $1 AND enabled RETURNING 1),
                    a AS (UPDATE activity_log_settings SET enabled = false, disabled_reason = 'quack_left',
                                                   version = version + 1, updated_at = now(), updated_by = NULL
                          WHERE guild_id = $1 AND enabled RETURNING 1),
                    h AS (UPDATE honeypot_settings SET enabled = false, disabled_reason = 'quack_left',
                                                   version = version + 1, updated_at = now(), updated_by = NULL
                          WHERE guild_id = $1 AND enabled RETURNING 1)
               SELECT (SELECT count(*) FROM t) AS "tickets!", (SELECT count(*) FROM a) AS "activity!",
                      (SELECT count(*) FROM h) AS "honeypot!""#,
            i64::from(guild)
        )
        .fetch_one(conn),
    )
    .await?;
    let mut off = Vec::new();
    for (n, module) in [
        (row.tickets, ModuleName::Tickets),
        (row.activity, ModuleName::ActivityLog),
        (row.honeypot, ModuleName::Honeypot),
    ] {
        if n > 0 {
            off.push(module);
        }
    }
    Ok(off)
}

/// Turns a module on or off (§13.1.7) if it is not already in that state:
/// `enabled`, `disabled_reason` (only when turning off: `None` for staff,
/// or why Quack turned it off), `version + 1`. Returns the new version, or
/// `None` when nothing changed.
pub async fn set_module_enabled(
    conn: &mut PgConnection,
    guild: GuildId,
    module: ModuleName,
    enabled: bool,
    reason: Option<quack_core::modules::ModuleDisabledReason>,
    by: Option<AccountId>,
) -> Result<Option<u64>, DbError> {
    let reason = if enabled {
        None
    } else {
        reason.map(crate::pg::Pg)
    };
    let g = i64::from(guild);
    let by = by.map(i64::from);
    let version = match module {
        ModuleName::Tickets => {
            obs::run(
                Update,
                "ticket_settings",
                sqlx::query_scalar!(
                    "UPDATE ticket_settings SET enabled = $2, disabled_reason = $3, version = version + 1,
                                                updated_at = now(), updated_by = $4
                     WHERE guild_id = $1 AND enabled <> $2 RETURNING version",
                    g,
                    enabled,
                    reason as _,
                    by
                )
                .fetch_optional(conn),
            )
            .await?
        }
        ModuleName::ActivityLog => {
            obs::run(
                Update,
                "activity_log_settings",
                sqlx::query_scalar!(
                    "UPDATE activity_log_settings SET enabled = $2, disabled_reason = $3, version = version + 1,
                                                      updated_at = now(), updated_by = $4
                     WHERE guild_id = $1 AND enabled <> $2 RETURNING version",
                    g,
                    enabled,
                    reason as _,
                    by
                )
                .fetch_optional(conn),
            )
            .await?
        }
        ModuleName::Honeypot => {
            obs::run(
                Update,
                "honeypot_settings",
                sqlx::query_scalar!(
                    "UPDATE honeypot_settings SET enabled = $2, disabled_reason = $3, version = version + 1,
                                                  updated_at = now(), updated_by = $4
                     WHERE guild_id = $1 AND enabled <> $2 RETURNING version",
                    g,
                    enabled,
                    reason as _,
                    by
                )
                .fetch_optional(conn),
            )
            .await?
        }
    };
    version.map(count).transpose()
}
