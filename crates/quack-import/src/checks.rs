//! Relationship checks run inside each server's transaction before it
//! commits (or rolls back in a dry run), so a dry run checks them too
//! (§19.10). Most are also enforced by foreign keys; checking them again
//! makes the report say so explicitly.

use sqlx::Row as _;

use crate::ImportError;
use crate::guild::Ctx;

/// `(name, SQL returning failing keys as text; $1 is the server)`.
const CHECKS: &[(&str, &str)] = &[
    (
        "case_rule_version_exists",
        "SELECT c.id::text FROM cases c LEFT JOIN rule_versions v ON v.rule_id = c.rule_id AND v.version = c.rule_version
         WHERE c.guild_id = $1 AND c.rule_id IS NOT NULL AND v.rule_id IS NULL",
    ),
    (
        "action_matches_case",
        "SELECT a.id::text FROM actions a LEFT JOIN cases c ON c.id = a.case_id
         WHERE a.guild_id = $1 AND (c.id IS NULL OR c.guild_id <> a.guild_id OR c.member_id <> a.member_id)",
    ),
    (
        "undo_matches_original_case",
        "SELECT a.id::text FROM actions a JOIN actions o ON o.id = a.undo_of_action_id
         WHERE a.guild_id = $1 AND o.case_id <> a.case_id",
    ),
    (
        "replacement_same_server_and_voided",
        "SELECT c.id::text FROM cases c JOIN cases r ON r.id = c.replaces_case_id
         WHERE c.guild_id = $1 AND (r.guild_id <> c.guild_id OR r.voided_at IS NULL)",
    ),
    (
        "replacement_acyclic",
        "WITH RECURSIVE walk(start, at, depth) AS (
           SELECT id, replaces_case_id, 1 FROM cases WHERE guild_id = $1 AND replaces_case_id IS NOT NULL
           UNION ALL
           SELECT w.start, c.replaces_case_id, w.depth + 1 FROM walk w JOIN cases c ON c.id = w.at
           WHERE c.replaces_case_id IS NOT NULL AND w.at <> w.start AND w.depth < 10000)
         SELECT DISTINCT start::text FROM walk WHERE at = start",
    ),
    (
        "appeal_matches_case",
        "SELECT a.id::text FROM appeals a JOIN cases c ON c.id = a.case_id
         WHERE a.guild_id = $1 AND (c.guild_id <> a.guild_id OR c.member_id <> a.member_id)",
    ),
    (
        "dm_member_matches",
        "SELECT d.id::text FROM dms d LEFT JOIN cases c ON c.id = d.case_id LEFT JOIN tickets t ON t.id = d.ticket_id
         WHERE d.guild_id = $1 AND d.member_id IS DISTINCT FROM COALESCE(c.member_id, t.member_id)",
    ),
    (
        "case_counter_at_least_max",
        "SELECT k.guild_id::text FROM case_counters k
         WHERE k.guild_id = $1 AND k.last_number < (SELECT COALESCE(max(number), 0) FROM cases WHERE guild_id = $1)",
    ),
    (
        "no_job_for_uncertain_work",
        "SELECT j.key FROM jobs j
         LEFT JOIN actions a ON j.kind = 'run_action' AND a.id = (j.payload->>'action_id')::uuid
         LEFT JOIN dms d ON j.kind = 'send_dm' AND d.id = (j.payload->>'dm_id')::uuid
         WHERE j.guild_id = $1 AND ((j.kind = 'run_action' AND (a.id IS NULL OR a.state <> 'pending'))
                                 OR (j.kind = 'send_dm' AND (d.id IS NULL OR d.state <> 'pending')))",
    ),
    (
        "no_open_attempt",
        "SELECT action_id::text FROM action_attempts WHERE guild_id = $1 AND finished_at IS NULL",
    ),
];

/// Runs every check for the server in its transaction.
pub(crate) async fn run(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    for (name, sql) in CHECKS {
        let rows = sqlx::query(*sql)
            .bind(cx.g.id)
            .fetch_all(&mut *cx.tx)
            .await
            .map_err(ImportError::target)?;
        let failing = rows
            .iter()
            .map(|r| {
                r.try_get::<String, _>(0)
                    .map(|k| format!("{}:{k}", cx.g.id))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(ImportError::target)?;
        cx.f.check(name, failing);
    }
    // Every source case of the server was written (or reported).
    let written: i64 = sqlx::query_scalar("SELECT count(*) FROM cases WHERE guild_id = $1")
        .bind(cx.g.id)
        .fetch_one(&mut *cx.tx)
        .await
        .map_err(ImportError::target)?;
    let source = cx
        .src
        .rows(
            "SELECT COUNT(*) AS n FROM cases WHERE guild_id = ?",
            &[&cx.g.ulid.clone()],
        )
        .await?;
    let source: i64 = source
        .first()
        .map(|r| r.try_get("n"))
        .transpose()
        .map_err(ImportError::source)?
        .unwrap_or(0);
    let skipped =
        cx.f.skipped
            .get("cases")
            .map_or(0, |m| m.values().map(|e| e.count).sum::<u64>());
    let failing = if u64::try_from(source - written).ok() == Some(skipped) {
        Vec::new()
    } else {
        vec![format!(
            "{}: source {source}, written {written}, skipped {skipped}",
            cx.g.id
        )]
    };
    cx.f.check("every_case_accounted_for", failing);
    Ok(())
}
