//! Source-wide accounting (§19.4, §19.10): tables that are not imported are
//! counted by category; rows no server scan can reach (orphans) are
//! reported as skipped; links across servers are checked.

use sqlx::Row as _;

use crate::ImportError;
use crate::report::{Check, Examples, Report, nested};
use crate::source::Source;

/// `(table, category column or NULL)` for tables that are only counted.
const COUNTED: &[(&str, Option<&str>)] = &[
    ("staff_members", None),
    ("case_events", Some("event_type")),
    ("audit_mirror_deliveries", Some("status")),
    ("ticket_events", Some("event_type")),
    ("ticket_member_states", None),
    ("honeypot_warning_refreshes", None),
    ("v4_import_batches", None),
    ("v4_import_sources", None),
    ("guild_appeal_settings", None),
];

/// Rows no server scan reaches: `(table, key column, condition)`.
const ORPHANS: &[(&str, &str, &str)] = &[
    (
        "guild_settings",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "case_templates",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "case_template_context_fields",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM case_templates t JOIN guilds g ON g.id = t.guild_id WHERE t.id = x.template_id)",
    ),
    (
        "case_template_levels",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM case_templates t JOIN guilds g ON g.id = t.guild_id WHERE t.id = x.template_id)",
    ),
    (
        "case_template_level_actions",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM case_template_levels l JOIN case_templates t ON t.id = l.template_id WHERE l.id = x.level_id)",
    ),
    (
        "cases",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "case_action_executions",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM cases c JOIN guilds g ON g.id = c.guild_id WHERE c.id = x.case_id)",
    ),
    (
        "case_action_attempts",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM case_action_executions e WHERE e.id = x.execution_id)",
    ),
    (
        "case_notifications",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM cases c JOIN guilds g ON g.id = c.guild_id WHERE c.id = x.case_id)",
    ),
    (
        "case_publications",
        "x.message_id",
        "NOT EXISTS (SELECT 1 FROM cases c JOIN guilds g ON g.id = c.guild_id WHERE c.id = x.case_id)",
    ),
    (
        "case_evidence_snapshots",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "case_evidence_attachments",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM case_evidence_snapshots s WHERE s.id = x.evidence_id)",
    ),
    (
        "appeals",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "appeal_events",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "appeal_notifications",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "audit_log_entries",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "module_configurations",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "tickets",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "ticket_message_journal",
        "x.message_discord_id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "ticket_transcripts",
        "x.ticket_id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "honeypot_triggers",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
    (
        "honeypot_message_cleanups",
        "x.id",
        "NOT EXISTS (SELECT 1 FROM guilds g WHERE g.id = x.guild_id)",
    ),
];

/// Children whose own `guild_id` differs from their parent's:
/// `(child, key, parent join, parent guild column)`.
const CROSS: &[(&str, &str, &str)] = &[
    (
        "case_evidence_snapshots",
        "x.id",
        "JOIN cases p ON p.id = x.case_id",
    ),
    ("appeals", "x.id", "JOIN cases p ON p.id = x.case_id"),
    (
        "appeal_events",
        "x.id",
        "JOIN appeals p ON p.id = x.appeal_id",
    ),
    (
        "appeal_notifications",
        "x.id",
        "JOIN appeals p ON p.id = x.appeal_id",
    ),
    (
        "ticket_message_journal",
        "x.message_discord_id",
        "JOIN tickets p ON p.id = x.ticket_id",
    ),
    (
        "ticket_transcripts",
        "x.ticket_id",
        "JOIN tickets p ON p.id = x.ticket_id",
    ),
    (
        "honeypot_triggers",
        "x.id",
        "JOIN cases p ON p.id = x.case_id",
    ),
    ("cases", "x.id", "JOIN cases p ON p.id = x.replaces_case_id"),
];

/// Counts, orphans, and cross-server links, into the report.
pub(crate) async fn run(src: &mut Source, report: &mut Report) -> Result<(), ImportError> {
    for (table, category) in COUNTED {
        if !src.shape.has_table(table) {
            continue;
        }
        let sql = match category {
            Some(c) => format!(
                "SELECT CAST(`{c}` AS CHAR) AS k, COUNT(*) AS n FROM `{table}` GROUP BY `{c}`"
            ),
            None => format!("SELECT 'rows' AS k, COUNT(*) AS n FROM `{table}`"),
        };
        for row in src.rows(&sql, &[]).await? {
            let k: String = row
                .try_get::<Option<String>, _>("k")
                .map_err(ImportError::source)?
                .unwrap_or_default();
            let n: i64 = row.try_get("n").map_err(ImportError::source)?;
            nested(&mut report.not_imported, table, &k).count += n.unsigned_abs();
        }
    }
    in_flight(src, report).await?;
    for (table, key, condition) in ORPHANS {
        let sql = format!(
            "SELECT CAST({key} AS CHAR) AS k FROM `{table}` x WHERE {condition} ORDER BY {key}"
        );
        for row in src.rows(&sql, &[]).await? {
            let k: String = row.try_get("k").map_err(ImportError::source)?;
            nested(&mut report.skipped, table, "missing_parent").add(format!("orphan:{k}"));
        }
    }
    let mut cross = Check {
        ok: true,
        failing: Examples::default(),
    };
    for (table, key, join) in CROSS {
        let sql = format!(
            "SELECT CAST({key} AS CHAR) AS k FROM `{table}` x {join} WHERE p.guild_id <> x.guild_id ORDER BY {key}"
        );
        for row in src.rows(&sql, &[]).await? {
            let k: String = row.try_get("k").map_err(ImportError::source)?;
            cross.ok = false;
            cross.failing.add(format!("{table}:{k}"));
        }
    }
    report.add_check("source_cross_guild_links", cross);
    Ok(())
}

/// §19.9 rows that are dropped rather than imported.
async fn in_flight(src: &mut Source, report: &mut Report) -> Result<(), ImportError> {
    let queries = [
        (
            "audit_mirror_deliveries",
            "mirror_delivery_dropped",
            "SELECT audit_entry_id AS k FROM audit_mirror_deliveries WHERE status NOT IN ('delivered', 'skipped') ORDER BY audit_entry_id",
        ),
        (
            "ticket_member_states",
            "ticket_slot_reservation_dropped",
            "SELECT s.id AS k FROM ticket_member_states s WHERE s.open_ticket_id <> ''
             AND NOT EXISTS (SELECT 1 FROM tickets t WHERE t.id = s.open_ticket_id) ORDER BY s.id",
        ),
        (
            "honeypot_warning_refreshes",
            "warning_refresh_dropped",
            "SELECT guild_id AS k FROM honeypot_warning_refreshes WHERE pending = 1 ORDER BY guild_id",
        ),
    ];
    for (table, row_name, sql) in queries {
        if !src.shape.has_table(table) {
            continue;
        }
        for row in src.rows(sql, &[]).await? {
            let k: String = row.try_get("k").map_err(ImportError::source)?;
            report
                .in_flight
                .entry(row_name.to_owned())
                .or_default()
                .add(k);
        }
    }
    Ok(())
}
