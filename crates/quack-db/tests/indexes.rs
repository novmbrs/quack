//! Hot-path queries (§15.9) use their intended indexes.
//!
//! The escalation count, the claim, and the idle sweep are explained from
//! their production text in `.sqlx/`, planned as the pool plans them
//! (custom plans, sequential scans allowed) on analyzed seed data. The
//! other shapes are hand-written with literal values and explained with
//! sequential scans disabled, so the seed tables still show which index
//! the planner picks.

mod common;

use anyhow::Result;
use common::{db, guild, rule};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

const PLANS: &[(&str, &str)] = &[
    (
        "SELECT count(*) FROM cases WHERE guild_id = 1 AND member_id = 2
           AND rule_id = '00000000-0000-7000-8000-000000000000' AND voided_at IS NULL AND source <> 'imported'
           AND created_at >= '-infinity'",
        "cases_escalation",
    ),
    ("SELECT id FROM cases WHERE guild_id = 1 AND number = 5", "cases_guild_id_number_key"),
    (
        "SELECT id FROM cases WHERE member_id = 2 ORDER BY created_at DESC, id DESC LIMIT 50",
        "cases_member_all",
    ),
    (
        "SELECT id FROM cases WHERE guild_id = 1 AND member_id = 2 ORDER BY number DESC LIMIT 50",
        "cases_member",
    ),
    (
        // The undo guard (§5.6). "Another action of this member is running"
        // may equally use `actions_guild_state` (few rows are running).
        "SELECT a.id FROM actions a WHERE a.guild_id = 1 AND a.member_id = 2 AND a.kind = 'kick'
           AND a.state = 'done'",
        "actions_member",
    ),
    (
        "SELECT id FROM actions WHERE guild_id = 1 AND state = 'failed' AND dismissed_at IS NULL
           ORDER BY created_at DESC, id DESC LIMIT 50",
        "actions_review",
    ),
    (
        "SELECT id FROM jobs WHERE kind = 'send_dm' AND dead_at IS NULL AND locked_until IS NULL
           AND run_at <= now() ORDER BY run_at, id LIMIT 4 FOR UPDATE SKIP LOCKED",
        "jobs_runnable",
    ),
    (
        "SELECT id FROM jobs WHERE locked_until < now() AND dead_at IS NULL",
        "jobs_leased",
    ),
    (
        "SELECT response FROM operation_keys WHERE guild_id = 1 AND account_id = 2 AND operation = 'x' AND key = 'y'",
        "operation_keys_pkey",
    ),
    (
        "SELECT account_id FROM sessions WHERE id_hash = '\\x00' AND expires_at > now() AND key_fingerprint = '\\x00'",
        "sessions_pkey",
    ),
    (
        "SELECT id FROM mod_log WHERE guild_id = 1 ORDER BY created_at DESC, id DESC LIMIT 50",
        "mod_log_guild_time",
    ),
    (
        "SELECT id FROM mod_log WHERE case_id = '00000000-0000-7000-8000-000000000000' ORDER BY created_at, id",
        "mod_log_case",
    ),
    (
        "SELECT id FROM appeals WHERE guild_id = 1 AND state = 'pending' ORDER BY created_at, id LIMIT 25",
        "appeals_guild_state",
    ),
    (
        "SELECT id FROM messages WHERE channel_id = 1 AND message_id = 2",
        "messages_location",
    ),
    (
        "SELECT id FROM tickets WHERE guild_id = 1 AND member_id = 2 AND state IN ('opening', 'open', 'closing')",
        "tickets_one_live",
    ),
    (
        "SELECT message_id FROM honeypot_hits WHERE guild_id = 1 AND member_id = 2 AND state = 'pending'
           ORDER BY message_id",
        "honeypot_hits_pending",
    ),
    (
        "SELECT last_number FROM case_counters WHERE guild_id = 1 FOR UPDATE",
        "case_counters_pkey",
    ),
];

/// Hot paths checked with their production text: a substring that picks
/// the query from `.sqlx/`, its parameter values, and the index.
const PRODUCTION: &[(&str, &str, &str)] = &[
    (
        "AND source <> 'imported' AND created_at >= COALESCE($4",
        "1, 3, '{rule}', NULL",
        "cases_escalation",
    ),
    ("WITH picked AS (", "'send_dm', 8, 60", "jobs_runnable"),
    ("WITH swept AS (", "'send_dm'", "jobs_leased"),
];

/// The production text of the one query in `.sqlx/` containing `marker`.
fn production_query(marker: &str) -> Result<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sqlx");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let text = std::fs::read_to_string(entry?.path())?;
        let data: serde_json::Value = serde_json::from_str(&text)?;
        if let Some(query) = data["query"].as_str()
            && query.contains(marker)
        {
            found.push(query.to_owned());
        }
    }
    anyhow::ensure!(found.len() == 1, "{} queries match {marker:?}", found.len());
    Ok(found.remove(0))
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn hot_path_queries_use_their_indexes(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let mut conn = db.conn().await?;
    // Enough rows that the planner's costs tell indexes apart.
    sqlx::query(
        "INSERT INTO cases (guild_id, number, member_id, source, moderator_id, rule_id, rule_version, case_count,
                            level_index, level_from_case_count, sends_dm, created_at)
         SELECT 1, n, 1 + n % 500, 'discord'::case_source, 9, $1, 1, 1, 1, 1, false, now() - n * interval '1 minute'
         FROM generate_series(1, 5000) AS n
         UNION ALL
         -- One member with many cases, as a member's own list is meant for.
         SELECT 1, n, 2, 'discord', 9, $1, 1, 1, 1, 1, false, now() - n * interval '1 second'
         FROM generate_series(5001, 8000) AS n",
    )
    .bind(rule_id.get())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO mod_log (guild_id, kind, result, source, rule_id, created_at)
         SELECT 1, 'rule_edited', 'ok', 'dashboard', $1, now() - n * interval '1 minute'
         FROM generate_series(1, 5000) AS n",
    )
    .bind(rule_id.get())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO jobs (kind, key, guild_id, max_attempts, run_at)
         SELECT CASE WHEN n % 2 = 0 THEN 'send_dm'::job_kind ELSE 'sync_message' END,
                CASE WHEN n % 2 = 0 THEN 'send_dm:' ELSE 'sync_message:' END || n, 1, 3,
                now() - n * interval '1 second'
         FROM generate_series(1, 5000) AS n",
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO actions (guild_id, case_id, member_id, kind, state, done_at, failure_reason, created_at)
         SELECT guild_id, id, member_id, 'kick', CASE WHEN number % 10 = 0 THEN 'failed'::action_state ELSE 'done' END,
                CASE WHEN number % 10 = 0 THEN NULL ELSE now() END,
                CASE WHEN number % 10 = 0 THEN 'discord_refused'::action_failure END, created_at
         FROM cases WHERE guild_id = 1",
    )
    .execute(&mut *conn)
    .await?;
    for seed in [
        "INSERT INTO appeals (guild_id, case_id, member_id, state, statement, created_at)
         SELECT guild_id, id, member_id, CASE WHEN number % 30 = 0 THEN 'pending'::appeal_state ELSE 'rejected' END,
                'please', created_at
         FROM cases WHERE number % 3 = 0",
        "INSERT INTO messages (guild_id, kind, case_id, channel_id, message_id)
         SELECT guild_id, 'case_receipt', id, 1 + number % 7, number FROM cases",
        "INSERT INTO tickets (guild_id, member_id, state, entry_channel_id, closed_at)
         SELECT 1, n, CASE WHEN n % 50 = 0 THEN 'opening'::ticket_state ELSE 'closed' END, 5,
                CASE WHEN n % 50 = 0 THEN NULL ELSE now() END
         FROM generate_series(1, 3000) AS n",
        "INSERT INTO honeypot_hits (guild_id, message_id, channel_id, member_id, settings_version, state, failure)
         SELECT 1, n, 5, 1 + n % 400, 1, CASE WHEN n % 40 = 0 THEN 'pending'::honeypot_hit_state ELSE 'failed' END,
                CASE WHEN n % 40 = 0 THEN NULL ELSE 'exempt' END
         FROM generate_series(1, 5000) AS n",
    ] {
        sqlx::query(seed).execute(&mut *conn).await?;
    }
    sqlx::query("ANALYZE").execute(&mut *conn).await?;
    // The production query texts (from the offline data the build checks),
    // planned the way the pool plans them: custom plans, sequential scans
    // allowed.
    for (marker, values, index) in PRODUCTION {
        let sql = production_query(marker)?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("PREPARE hot AS {sql}")))
            .execute(&mut *conn)
            .await?;
        let values = values.replace("{rule}", &rule_id.get().to_string());
        let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN EXECUTE hot({values})"
        )))
        .fetch_all(&mut *conn)
        .await?;
        sqlx::raw_sql("DEALLOCATE hot").execute(&mut *conn).await?;
        let plan = plan.join("\n");
        assert!(
            plan.contains(index),
            "{marker}\nexpected {index}, got:\n{plan}"
        );
    }
    sqlx::query("SET enable_seqscan = off")
        .execute(&mut *conn)
        .await?;
    for (statement, index) in PLANS {
        let plan: Vec<String> =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("EXPLAIN {statement}")))
                .fetch_all(&mut *conn)
                .await?;
        let plan = plan.join("\n");
        assert!(
            plan.contains(index),
            "{statement}\nexpected {index}, got:\n{plan}"
        );
    }
    Ok(())
}
