//! The source manifest (§19.1): every Go table and column the importer
//! accepts, and its fate. Before reading data the importer compares it with
//! `information_schema.columns` and refuses an unknown shape.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sqlx::{MySqlConnection, Row as _};

use crate::ImportError;

/// Bumped whenever an entry below changes; part of the fingerprint.
pub const MANIFEST_VERSION: u32 = 1;

/// The accepted ledger names, in order (§19.1).
pub const LEDGER: [&str; 5] = [
    "baseline",
    "audit_mirror_deliveries",
    "launch_announcement",
    "staff_roles",
    "discord_user_mfa",
];

/// What happens to a column.
#[derive(Clone, Copy, Debug)]
enum Fate {
    /// Imported into this target (a §19 rule names how).
    To(&'static str),
    /// Not imported, for this reason.
    Drop(&'static str),
}
use Fate::{Drop, To};

/// When a column (or table) exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Since {
    /// In every accepted ledger.
    Always,
    /// From this ledger version on; tolerated earlier.
    Ledger(u8),
    /// A legacy leftover the Go startup cleanup may not have removed.
    Legacy,
}

const NOT_IMPORTED: Fate = Drop("table not imported; counted in the report");

type Columns = &'static [(&'static str, Since, Fate)];

const A: Since = Since::Always;

#[rustfmt::skip]
const TABLES: &[(&str, Since, Columns)] = &[
    ("quack_schema_migrations", A, &[
        ("version", A, To("report.source.ledger")), ("name", A, To("report.source.ledger")),
        ("applied_at", A, Drop("ledger time not needed")),
    ]),
    ("guilds", A, &[
        ("id", A, Drop("children map the internal ULID to the Discord server ID")),
        ("created_at", A, To("guilds.first_joined_at")), ("updated_at", A, To("guilds.updated_at")),
        ("discord_guild_id", A, To("guilds.guild_id")), ("name", A, To("guilds.name")),
        ("icon_url", A, To("guilds.icon_hash")), ("owner_discord_user_id", A, To("guilds.owner_id")),
        ("is_active", A, To("guilds.installed")),
    ]),
    ("guild_settings", A, &[
        ("id", A, Drop("one row per server")), ("created_at", A, Drop("no target column")),
        ("updated_at", A, To("guild_settings.updated_at")), ("guild_id", A, To("guild_settings.guild_id")),
        ("audit_mirror_channel_discord_id", A, To("guild_settings.log_channel_id")),
        ("managed_evidence_channel_discord_id", A, To("guild_settings.evidence_channel_id")),
        ("notification_introduction", A, To("guild_settings.dm_intro")),
        ("notification_footer", A, To("guild_settings.dm_footer")),
        ("starter_policy_template_id", A, To("guild_settings.starter_rule_id")),
        ("starter_policy_notice_pending", A, Drop("derived from starter_notice_acknowledged_at")),
        ("starter_policy_notice_acknowledged_at", A, To("guild_settings.starter_notice_acknowledged_at")),
        ("appeal_queue_channel_discord_id", A, To("guild_settings.appeal_channel_id")),
        ("appeal_rejoin_url", A, To("guild_settings.appeal_invite_url")),
        ("appeal_review_reason_required", A, Drop("a decision reason is always required")),
        ("launch_announced_at", Since::Ledger(3), Drop("launch announcement dropped")),
        ("moderator_role_ids", Since::Ledger(4), To("guild_settings.moderator_role_ids")),
        ("rules_manager_role_ids", Since::Ledger(4), To("guild_settings.rules_manager_role_ids")),
    ]),
    ("staff_members", A, &[
        ("id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("updated_at", A, NOT_IMPORTED),
        ("guild_id", A, NOT_IMPORTED), ("discord_user_id", A, NOT_IMPORTED),
        ("last_seen_permission_bits", A, NOT_IMPORTED), ("last_known_display_name", A, NOT_IMPORTED),
        ("last_active_at", A, NOT_IMPORTED),
    ]),
    ("discord_user_mfa", Since::Ledger(5), &[
        ("discord_user_id", A, To("discord_accounts.account_id")), ("created_at", A, Drop("no target column")),
        ("updated_at", A, To("discord_accounts.updated_at")), ("mfa_enabled", A, To("discord_accounts.mfa_enabled")),
        ("mfa_checked_at", A, To("discord_accounts.mfa_checked_at")),
    ]),
    ("case_templates", A, &[
        ("id", A, To("rules.id")), ("created_at", A, To("rules.created_at")),
        ("updated_at", A, To("rules.updated_at, rule_versions.created_at")), ("guild_id", A, To("rules.guild_id")),
        ("slug", A, Drop("rules have no slug")), ("name", A, To("rules.name, definition.name")),
        ("description", A, To("definition.description")), ("reason_template", A, To("definition.reason")),
        ("appealable", A, To("definition.appealable")), ("version", A, To("rules.current_version")),
        ("created_by_discord_user_id", A, To("rules.created_by")),
        ("updated_by_discord_user_id", A, To("rules.updated_by, rule_versions.created_by")),
        ("archived_at", A, To("rules.archived_at")), ("case_decay_days", A, To("definition.counting_window_days")),
    ]),
    ("case_template_context_fields", A, &[
        ("id", A, Drop("child IDs are not kept")), ("created_at", A, Drop("child timestamps are not kept")),
        ("updated_at", A, Drop("child timestamps are not kept")), ("template_id", A, To("rule_versions.rule_id")),
        ("key", A, To("definition.fields[].key")), ("label", A, To("definition.fields[].label")),
        ("field_type", A, To("definition.fields[].kind")), ("position", A, To("definition.fields order")),
        ("required", A, To("definition.fields[].required")),
    ]),
    ("case_template_levels", A, &[
        ("id", A, Drop("child IDs are not kept")), ("created_at", A, Drop("child timestamps are not kept")),
        ("updated_at", A, Drop("child timestamps are not kept")), ("template_id", A, To("rule_versions.rule_id")),
        ("position", A, To("definition.levels order")), ("name", A, Drop("levels have no name")),
        ("is_default", A, To("definition.levels[0].from_case_count = 1")),
        ("trigger_case_count", A, To("definition.levels[].from_case_count")),
        ("notify_user", A, To("definition.levels[].send_dm")),
        ("default_template_id", A, Drop("generated column; checked")),
    ]),
    ("case_template_level_actions", A, &[
        ("id", A, Drop("child IDs are not kept")), ("created_at", A, Drop("child timestamps are not kept")),
        ("updated_at", A, Drop("child timestamps are not kept")), ("level_id", A, To("definition.levels[]")),
        ("action_type", A, To("definition.levels[].action.kind")),
        ("config_json", A, To("definition.levels[].action")), ("max_retries", A, Drop("retry policy is fixed")),
    ]),
    ("cases", A, &[
        ("id", A, To("cases.id")), ("created_at", A, To("cases.created_at")), ("updated_at", A, To("cases.updated_at")),
        ("guild_id", A, To("cases.guild_id")), ("case_number", A, To("cases.number")),
        ("template_id", A, To("cases.rule_id")), ("template_version", A, To("cases.rule_version")),
        ("template_snapshot_json", A, To("cases decision, reconstructed rule_versions")),
        ("target_discord_user_id", A, To("cases.member_id")), ("moderator_discord_user_id", A, To("cases.moderator_id")),
        ("reason", A, To("rule version reason; cases.imported.v4_reason for v4 cases")),
        ("validity", A, To("cases.voided_at (checked)")), ("source", A, To("cases.source")),
        ("correlation_id", A, Drop("not a trace ID")),
        ("context_channel_discord_id", A, Drop("its message is an evidence row")),
        ("context_message_discord_id", A, Drop("its message is an evidence row")),
        ("context_url", A, Drop("its message is an evidence row")),
        ("metadata_json", A, To("cases.imported (v4 only); otherwise dropped")),
        ("context_values_json", A, To("cases.context")), ("voided_reason", A, To("cases.void_reason")),
        ("voided_by_discord_user_id", A, To("cases.voided_by")), ("voided_at", A, To("cases.voided_at")),
        ("replacement_case_id", A, Drop("the inverse of replaces_case_id; checked")),
        ("replaces_case_id", A, To("cases.replaces_case_id")), ("idempotency_key", A, To("cases.operation_key")),
    ]),
    ("case_action_executions", A, &[
        ("id", A, To("actions.id")), ("created_at", A, To("actions.created_at")), ("updated_at", A, To("actions.updated_at")),
        ("case_id", A, To("actions.case_id")), ("template_action_id", A, Drop("rule children are not kept")),
        ("position", A, Drop("one action per case")), ("action_type", A, To("actions.kind")),
        ("status", A, To("actions.state")), ("idempotency_key", A, Drop("actions are keyed by ID")),
        ("config_snapshot_json", A, To("actions.delete_message_seconds, requested_by, timeout_ends_at")),
        ("notify_user", A, Drop("DMs are their own rows")), ("notification_type", A, Drop("DMs are their own rows")),
        ("attempt_count", A, Drop("attempts restart at cutover")), ("max_retries", A, Drop("retry policy is fixed")),
        ("retry_backoff_ms", A, Drop("retry policy is fixed")), ("safe_for_retry", A, Drop("every action is idempotent")),
        ("last_error_code", A, To("actions.state, failure_reason")), ("last_error", A, Drop("free text")),
        ("started_at", A, Drop("attempts hold their own times")), ("finished_at", A, To("actions.done_at")),
        ("next_retry_at", A, Drop("pending actions run at cutover")), ("correlation_id", A, Drop("not a trace ID")),
        ("lease_token", A, Drop("leases are not kept")), ("lease_expires_at", A, Drop("leases are not kept")),
        ("dismissed_at", A, To("actions.dismissed_at")), ("dismissed_by_discord_user_id", A, To("actions.dismissed_by")),
        ("reversal_of_execution_id", A, To("actions.undo_of_action_id")),
        ("reversal_appeal_id", A, Drop("the appeal is linked through the case")),
    ]),
    ("case_action_attempts", A, &[
        ("id", A, Drop("attempts are keyed by action and number")), ("created_at", A, Drop("started_at is kept")),
        ("updated_at", A, Drop("finished_at is kept")), ("execution_id", A, To("action_attempts.action_id")),
        ("attempt_number", A, To("action_attempts.number")), ("status", A, To("action_attempts.outcome")),
        ("worker_id", A, Drop("not needed")), ("started_at", A, To("action_attempts.started_at")),
        ("finished_at", A, To("action_attempts.finished_at")), ("duration_ms", A, Drop("derived from the times")),
        ("error_code", A, To("action_attempts.outcome")), ("error_message", A, Drop("free text")),
        ("request_payload_json", A, Drop("diagnostic only")),
        ("response_payload_json", A, To("actions.timeout_ends_at, actions.no_change, action_attempts.outcome")),
    ]),
    ("case_events", A, &[
        ("id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("updated_at", A, NOT_IMPORTED),
        ("case_id", A, NOT_IMPORTED), ("guild_id", A, NOT_IMPORTED), ("event_type", A, To("report.not_imported counts")),
        ("actor_discord_user_id", A, NOT_IMPORTED), ("actor_type", A, NOT_IMPORTED), ("visibility", A, NOT_IMPORTED),
        ("body", A, NOT_IMPORTED), ("metadata_json", A, NOT_IMPORTED),
    ]),
    ("case_evidence_snapshots", A, &[
        ("id", A, To("evidence.id")), ("created_at", A, To("evidence.created_at")), ("updated_at", A, To("evidence.saved_at")),
        ("case_id", A, To("evidence.case_id")), ("guild_id", A, To("evidence.guild_id")),
        ("channel_discord_id", A, To("evidence.channel_id")), ("message_discord_id", A, To("evidence.message_id")),
        ("author_discord_user_id", A, To("evidence.author_id")), ("message_url", A, Drop("derived from the IDs")),
        ("content", A, To("evidence.content")), ("message_created_at", A, To("evidence.message_created_at")),
        ("message_edited_at", A, To("evidence.message_edited_at")), ("embeds_json", A, To("evidence.embeds")),
        ("capture_outcome", A, To("evidence.state, failure")), ("capture_warning", A, Drop("free text")),
    ]),
    ("case_evidence_attachments", A, &[
        ("id", A, To("evidence_files.id")), ("created_at", A, To("evidence_files.created_at, position")),
        ("updated_at", A, Drop("no target column")), ("evidence_id", A, To("evidence_files.evidence_id")),
        ("filename", A, To("evidence_files.filename")), ("content_type", A, To("evidence_files.content_type")),
        ("size_bytes", A, To("evidence_files.size_bytes")), ("original_url", A, To("evidence_files.original_url, attachment_id")),
        ("preserved_url", A, To("evidence_files.copy_* (checked)")),
        ("preserved_message_discord_id", A, To("evidence_files.copy_message_id")),
        ("preserved_attachment_discord_id", A, To("evidence_files.copy_attachment_id")),
        ("copy_outcome", A, To("evidence_files.copy_state")), ("warning", A, Drop("free text")),
    ]),
    ("case_notifications", A, &[
        ("id", A, To("dms.id")), ("created_at", A, To("dms.created_at")), ("updated_at", A, To("dms.settled_at")),
        ("case_id", A, To("dms.case_id")), ("status", A, To("dms.state")),
        ("prepared_channel_discord_id", A, To("dms.channel_id")), ("rendered_message", A, Drop("re-rendered on send")),
        ("delivery_message_discord_id", A, To("dms.message_id")), ("attempt_count", A, Drop("not needed")),
        ("last_error_code", A, To("dms.state, failure")), ("last_error", A, Drop("free text")),
        ("lease_token", A, Drop("leases are not kept")), ("lease_expires_at", A, Drop("leases are not kept")),
        ("sent_at", A, To("dms.first_attempt_at, settled_at")),
    ]),
    ("case_publications", A, &[
        ("message_id", A, To("messages.message_id")), ("created_at", A, To("messages.created_at")),
        ("updated_at", A, To("messages.updated_at")), ("case_id", A, To("messages.case_id")),
        ("channel_id", A, To("messages.channel_id")), ("presentation_json", A, Drop("checked, then not needed")),
        ("last_digest", A, Drop("not needed")), ("refresh_requested", A, Drop("old receipts are not re-rendered; counted")),
        ("retry_at", A, Drop("not needed")), ("revision", A, Drop("revisions restart at 1")),
    ]),
    ("appeals", A, &[
        ("id", A, To("appeals.id")), ("created_at", A, To("appeals.created_at")), ("updated_at", A, To("appeals.updated_at")),
        ("guild_id", A, To("appeals.guild_id")), ("case_id", A, To("appeals.case_id")),
        ("target_discord_user_id", A, To("appeals.member_id (checked)")), ("status", A, To("appeals.state")),
        ("statement", Since::Legacy, To("appeals.statement")), ("version", A, To("appeals.version")),
        ("decision_reason", A, To("appeals.decision_reason")), ("reviewed_by_discord_user_id", A, To("appeals.decided_by")),
        ("reviewed_at", A, To("appeals.decided_at")), ("review_message_discord_id", A, To("messages (appeal_post)")),
        ("metadata_json", A, Drop("always empty")),
        ("content", Since::Legacy, To("appeals.statement (legacy column)")),
        ("answers_json", Since::Legacy, To("appeals.statement when empty (legacy form answers)")),
        ("question_snapshot_json", Since::Legacy, Drop("legacy appeal form")),
    ]),
    ("guild_appeal_settings", Since::Legacy, &[
        ("id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("updated_at", A, NOT_IMPORTED),
        ("guild_id", A, NOT_IMPORTED), ("questions_json", A, NOT_IMPORTED), ("updated_by_discord_user_id", A, NOT_IMPORTED),
    ]),
    ("appeal_events", A, &[
        ("id", A, To("appeal_events.id")), ("created_at", A, To("appeal_events.created_at")),
        ("updated_at", A, Drop("events are immutable")), ("appeal_id", A, To("appeal_events.appeal_id")),
        ("guild_id", A, To("appeal_events.guild_id")), ("event_type", A, To("appeal_events.kind")),
        ("actor_discord_user_id", A, To("appeal_events.actor_id")), ("actor_type", A, Drop("implied by the kind")),
        ("body", A, To("appeal_events.text")), ("metadata_json", A, To("appeal_events.closed_by_void")),
    ]),
    ("appeal_notifications", A, &[
        ("id", A, To("dms.id (member)")), ("created_at", A, To("dms.created_at, messages.created_at")),
        ("updated_at", A, To("dms.settled_at")), ("appeal_id", A, To("dms.case_id, messages.appeal_id")),
        ("event_id", A, To("dms.appeal_event_id")), ("guild_id", A, To("dms.guild_id")),
        ("target_discord_user_id", A, To("dms.member_id (checked)")), ("audience", A, To("dms or messages")),
        ("status", A, To("dms.state")), ("body", A, Drop("re-rendered")),
        ("delivery_message_id", A, To("dms.message_id, messages.message_id")),
        ("last_error_code", A, To("dms.state, failure")), ("lease_token", A, Drop("leases are not kept")),
        ("lease_expires_at", A, Drop("leases are not kept")), ("decision_intent_json", A, Drop("re-rendered")),
        ("delivery_channel_id", A, To("dms.channel_id, messages.channel_id")),
        ("refresh_requested", A, To("messages.wanted_revision")),
    ]),
    ("audit_log_entries", A, &[
        ("id", A, To("mod_log.id")), ("created_at", A, To("mod_log.created_at")), ("updated_at", A, Drop("append-only")),
        ("guild_id", A, To("mod_log.guild_id")), ("actor_discord_user_id", A, To("mod_log.actor_id")),
        ("actor_permission_bits", A, Drop("not needed")), ("source", A, Drop("imported entries have source imported")),
        ("action", A, To("mod_log.detail.action")), ("resource_type", A, To("mod_log.detail, typed links")),
        ("resource_id", A, To("mod_log.detail, typed links")), ("result", A, To("mod_log.result")),
        ("failure_reason", A, To("mod_log.detail.failure_reason (redacted)")),
        ("correlation_id", A, Drop("not a W3C trace ID")), ("request_id", A, Drop("not needed")),
        ("metadata_json", A, To("typed links; other keys dropped")),
    ]),
    ("audit_mirror_deliveries", Since::Ledger(2), &[
        ("audit_entry_id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("updated_at", A, NOT_IMPORTED),
        ("guild_id", A, NOT_IMPORTED), ("status", A, To("report.not_imported counts")), ("attempts", A, NOT_IMPORTED),
        ("next_attempt_at", A, NOT_IMPORTED), ("last_error", A, NOT_IMPORTED), ("delivered_message_id", A, NOT_IMPORTED),
        ("lease_token", A, NOT_IMPORTED), ("lease_expires_at", A, NOT_IMPORTED),
    ]),
    ("v4_import_batches", A, &[
        ("id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("guild_id", A, NOT_IMPORTED),
        ("source_name", A, NOT_IMPORTED), ("checksum", A, NOT_IMPORTED), ("actor_discord_user_id", A, NOT_IMPORTED),
        ("record_count", A, NOT_IMPORTED), ("created_count", A, NOT_IMPORTED),
        ("already_imported_count", A, NOT_IMPORTED), ("warning_count", A, NOT_IMPORTED),
    ]),
    ("v4_import_sources", A, &[
        ("id", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED), ("batch_id", A, NOT_IMPORTED),
        ("guild_id", A, NOT_IMPORTED), ("source_name", A, NOT_IMPORTED), ("source_id", A, NOT_IMPORTED),
        ("source_case_number", A, NOT_IMPORTED), ("target_case_id", A, NOT_IMPORTED), ("fingerprint", A, NOT_IMPORTED),
    ]),
    ("module_configurations", A, &[
        ("id", A, Drop("one row per server and module")), ("guild_id", A, To("*_settings.guild_id")),
        ("module_id", A, To("ticket_settings, activity_log_settings, honeypot_settings")),
        ("enabled", A, To("*_settings.enabled")), ("config_json", A, To("*_settings, activity_log_routes, messages")),
        ("created_at", A, Drop("no target column")), ("updated_at", A, To("*_settings.updated_at")),
    ]),
    ("tickets", A, &[
        ("id", A, To("tickets.id")), ("guild_id", A, To("tickets.guild_id")),
        ("owner_discord_user_id", A, To("tickets.member_id")), ("thread_discord_channel_id", A, To("tickets.thread_id")),
        ("status", A, To("tickets.state")), ("resolved_by_discord_user_id", A, To("tickets.closed_by")),
        ("resolved_at", A, To("tickets.closed_at")), ("metadata_json", A, To("dms (ticket_closed) from close_notice")),
        ("created_at", A, To("tickets.created_at, opened_at")), ("updated_at", A, To("tickets.updated_at")),
        ("log_channel_discord_id", A, To("messages (ticket_post).channel_id")),
        ("log_message_discord_id", A, To("messages (ticket_post).message_id")),
        ("queue_delivery_attempt_id", Since::Legacy, Drop("obsolete queue fence; non-empty values counted")),
        ("transcript_url", A, Drop("transcripts are their own rows")),
    ]),
    ("ticket_events", A, &[
        ("id", A, NOT_IMPORTED), ("ticket_id", A, NOT_IMPORTED), ("guild_id", A, NOT_IMPORTED),
        ("event_type", A, To("report.not_imported counts")), ("actor_discord_user_id", A, NOT_IMPORTED),
        ("body", A, NOT_IMPORTED), ("metadata_json", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED),
        ("updated_at", A, NOT_IMPORTED),
    ]),
    ("ticket_member_states", A, &[
        ("id", A, NOT_IMPORTED), ("guild_id", A, NOT_IMPORTED), ("owner_discord_user_id", A, NOT_IMPORTED),
        ("open_ticket_id", A, To("report.in_flight (reservations without a ticket)")),
        ("window_started_at", A, NOT_IMPORTED), ("open_count", A, NOT_IMPORTED), ("created_at", A, NOT_IMPORTED),
        ("updated_at", A, NOT_IMPORTED),
    ]),
    ("ticket_message_journal", A, &[
        ("guild_id", A, To("ticket_messages.guild_id")), ("thread_discord_channel_id", A, Drop("checked against the ticket")),
        ("message_discord_id", A, To("ticket_messages.message_id")), ("ticket_id", A, To("ticket_messages.ticket_id")),
        ("author_discord_user_id", A, To("ticket_messages.author_id")), ("author_name", A, To("ticket_messages.author_name")),
        ("body", A, To("ticket_messages.text")), ("sent_at", A, To("ticket_messages.sent_at")),
        ("expires_at", A, To("ticket_messages.expires_at")),
    ]),
    ("ticket_transcripts", A, &[
        ("ticket_id", A, To("ticket_transcripts.ticket_id")), ("guild_id", A, To("ticket_transcripts.guild_id")),
        ("content", A, To("ticket_transcripts.content")), ("captured_at", A, To("ticket_transcripts.captured_at")),
        ("expires_at", A, To("ticket_transcripts.expires_at")),
    ]),
    ("honeypot_triggers", A, &[
        ("id", A, Drop("hits are keyed by message")), ("guild_id", A, To("honeypot_hits.guild_id")),
        ("channel_discord_id", A, To("honeypot_hits.channel_id")), ("message_discord_id", A, To("honeypot_hits.message_id")),
        ("target_discord_user_id", A, To("honeypot_hits.member_id")), ("template_id", A, To("honeypot_hits.rule_id")),
        ("case_id", A, To("honeypot_hits.case_id")), ("outcome", A, To("honeypot_hits.state")),
        ("failure_code", A, To("honeypot_hits.failure")), ("created_at", A, To("honeypot_hits.created_at")),
        ("updated_at", A, To("honeypot_hits.updated_at")),
    ]),
    ("honeypot_message_cleanups", A, &[
        ("id", A, Drop("matched by message")), ("guild_id", A, To("honeypot_hits (match)")),
        ("message_discord_id", A, To("honeypot_hits (match)")), ("channel_discord_id", A, Drop("the hit has it")),
        ("target_discord_user_id", A, Drop("the hit has it")), ("trigger_id", A, Drop("matched by message")),
        ("attempt_count", A, Drop("not needed")), ("next_attempt_at", A, Drop("incomplete cleanups are enqueued")),
        ("completed_at", A, To("honeypot_hits.cleaned_up_at")), ("created_at", A, Drop("not needed")),
    ]),
    ("honeypot_warning_refreshes", A, &[
        ("guild_id", A, NOT_IMPORTED), ("revision", A, NOT_IMPORTED), ("pending", A, NOT_IMPORTED),
        ("next_attempt_at", A, NOT_IMPORTED), ("send_identity", A, NOT_IMPORTED),
    ]),
];

/// One source column and its fate, as the report lists it.
#[derive(Clone, Debug, Serialize)]
pub struct ManifestEntry {
    /// Source table.
    pub table: &'static str,
    /// Source column.
    pub column: &'static str,
    /// `mapped` or `dropped`.
    pub fate: &'static str,
    /// The target (mapped) or the reason (dropped).
    pub detail: &'static str,
    /// Whether this source has the column.
    pub present: bool,
}

/// What the source database looks like: its ledger and optional pieces.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    /// The highest ledger version (3, 4, or 5).
    pub ledger: u8,
    /// The ledger rows.
    pub ledger_rows: Vec<(u64, String)>,
    /// Present tables and their columns.
    pub columns: BTreeMap<String, BTreeSet<String>>,
}

impl Shape {
    /// Whether `table` exists.
    pub fn has_table(&self, table: &str) -> bool {
        self.columns.contains_key(table)
    }

    /// Whether `table.column` exists.
    pub fn has(&self, table: &str, column: &str) -> bool {
        self.columns.get(table).is_some_and(|c| c.contains(column))
    }

    /// The optional pieces present, for the report.
    pub fn features(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (table, since, columns) in TABLES {
            if *since != A && self.has_table(table) {
                out.push(format!("table {table}"));
            }
            for (column, since, _) in *columns {
                if *since != A && self.has(table, column) {
                    out.push(format!("{table}.{column}"));
                }
            }
        }
        out
    }

    /// The manifest for this source.
    pub fn manifest(&self) -> Vec<ManifestEntry> {
        let mut out = Vec::new();
        for (table, _, columns) in TABLES {
            for (column, _, fate) in *columns {
                let (fate, detail) = match fate {
                    To(t) => ("mapped", *t),
                    Drop(r) => ("dropped", *r),
                };
                out.push(ManifestEntry {
                    table,
                    column,
                    fate,
                    detail,
                    present: self.has(table, column),
                });
            }
        }
        out
    }
}

/// Reads the ledger and `information_schema`, and refuses any shape the
/// manifest does not describe.
pub(crate) async fn inspect(conn: &mut MySqlConnection) -> Result<Shape, ImportError> {
    let rows = sqlx::query(
        "SELECT table_name AS t, column_name AS c FROM information_schema.columns
         WHERE table_schema = DATABASE() ORDER BY table_name, ordinal_position",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(ImportError::source)?;
    let mut columns: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        let t: String = row.try_get("t").map_err(ImportError::source)?;
        let c: String = row.try_get("c").map_err(ImportError::source)?;
        columns.entry(t).or_default().insert(c);
    }
    if !columns.contains_key("quack_schema_migrations") {
        return Err(ImportError::Shape(vec![
            "quack_schema_migrations is missing: this is not a Quack database".to_owned(),
        ]));
    }
    let ledger_rows = read_ledger(conn).await?;
    let ledger = check_ledger(&ledger_rows)?;
    let shape = Shape {
        ledger,
        ledger_rows,
        columns,
    };
    check_columns(&shape)?;
    Ok(shape)
}

async fn read_ledger(conn: &mut MySqlConnection) -> Result<Vec<(u64, String)>, ImportError> {
    let rows = sqlx::query("SELECT version, name FROM quack_schema_migrations ORDER BY version")
        .fetch_all(conn)
        .await
        .map_err(ImportError::source)?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<u64, _>("version")
                    .map_err(ImportError::source)?,
                row.try_get::<String, _>("name")
                    .map_err(ImportError::source)?,
            ))
        })
        .collect()
}

/// The ledger must be `1–3`, `1–4`, or `1–5` with the expected names.
pub(crate) fn check_ledger(rows: &[(u64, String)]) -> Result<u8, ImportError> {
    let ok = (3..=5).contains(&rows.len())
        && rows
            .iter()
            .zip(LEDGER)
            .enumerate()
            .all(|(i, ((version, name), want))| *version == i as u64 + 1 && name == want);
    if ok {
        Ok(u8::try_from(rows.len()).unwrap_or(0))
    } else {
        Err(ImportError::Shape(vec![format!(
            "unsupported migration ledger {:?}; expected versions 1–3, 1–4, or 1–5 named {LEDGER:?}",
            rows.iter()
                .map(|(v, n)| format!("{v} {n}"))
                .collect::<Vec<_>>()
        )]))
    }
}

/// Every present table and column must be in the manifest, and every one
/// the ledger guarantees must be present.
pub(crate) fn check_columns(shape: &Shape) -> Result<(), ImportError> {
    let mut problems = Vec::new();
    let required = |since: Since| match since {
        Since::Always => true,
        Since::Ledger(v) => shape.ledger >= v,
        Since::Legacy => false,
    };
    for (table, present) in &shape.columns {
        let Some((_, _, columns)) = TABLES.iter().find(|(t, ..)| t == table) else {
            problems.push(format!("unexpected table {table}"));
            continue;
        };
        for column in present {
            if !columns.iter().any(|(c, ..)| c == column) {
                problems.push(format!("unexpected column {table}.{column}"));
            }
        }
    }
    for (table, since, columns) in TABLES {
        if !shape.has_table(table) {
            if required(*since) {
                problems.push(format!("missing table {table}"));
            }
            continue;
        }
        for (column, since, _) in *columns {
            if required(*since) && !shape.has(table, column) {
                problems.push(format!("missing column {table}.{column}"));
            }
        }
    }
    if !shape.has("appeals", "statement") && !shape.has("appeals", "content") {
        problems.push("appeals has neither statement nor content".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ImportError::Shape(problems))
    }
}

/// Every manifest table that exists in the source, in manifest order.
pub(crate) fn present_tables(shape: &Shape) -> Vec<&'static str> {
    TABLES
        .iter()
        .map(|(t, ..)| *t)
        .filter(|t| shape.has_table(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(ledger: u8) -> Shape {
        let mut columns = BTreeMap::new();
        for (table, since, cols) in TABLES {
            let table_present = match since {
                Since::Always => true,
                Since::Ledger(v) => ledger >= *v,
                Since::Legacy => false,
            };
            if !table_present {
                continue;
            }
            let set: BTreeSet<String> = cols
                .iter()
                .filter(|(c, s, _)| match s {
                    Since::Always => true,
                    Since::Ledger(v) => ledger >= *v,
                    Since::Legacy => *c == "statement",
                })
                .map(|(c, ..)| (*c).to_owned())
                .collect();
            columns.insert((*table).to_owned(), set);
        }
        Shape {
            ledger,
            ledger_rows: Vec::new(),
            columns,
        }
    }

    #[test]
    fn every_accepted_ledger_shape_passes() {
        for ledger in 3..=5 {
            check_columns(&shape(ledger)).unwrap();
        }
    }

    #[test]
    fn unexpected_and_missing_columns_abort() {
        let mut s = shape(5);
        s.columns
            .get_mut("cases")
            .unwrap()
            .insert("surprise".into());
        s.columns.get_mut("guilds").unwrap().remove("name");
        let Err(ImportError::Shape(problems)) = check_columns(&s) else {
            panic!("accepted a bad shape");
        };
        assert!(problems.contains(&"unexpected column cases.surprise".to_owned()));
        assert!(problems.contains(&"missing column guilds.name".to_owned()));
        let mut s = shape(5);
        s.columns.remove("discord_user_mfa");
        assert!(check_columns(&s).is_err());
        let mut s = shape(5);
        s.columns.insert("mystery".into(), BTreeSet::new());
        assert!(check_columns(&s).is_err());
    }

    #[test]
    fn legacy_appeal_columns_are_accepted() {
        let mut s = shape(3);
        let appeals = s.columns.get_mut("appeals").unwrap();
        appeals.remove("statement");
        for c in ["content", "answers_json", "question_snapshot_json"] {
            appeals.insert(c.into());
        }
        s.columns.insert(
            "guild_appeal_settings".into(),
            [
                "id",
                "created_at",
                "updated_at",
                "guild_id",
                "questions_json",
                "updated_by_discord_user_id",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        check_columns(&s).unwrap();
        assert!(s.features().contains(&"appeals.content".to_owned()));
    }

    #[test]
    fn ledgers() {
        let rows = |n: usize| -> Vec<(u64, String)> {
            LEDGER
                .iter()
                .take(n)
                .enumerate()
                .map(|(i, n)| (i as u64 + 1, (*n).to_owned()))
                .collect()
        };
        assert_eq!(check_ledger(&rows(3)).unwrap(), 3);
        assert_eq!(check_ledger(&rows(5)).unwrap(), 5);
        assert!(check_ledger(&rows(2)).is_err());
        let mut bad = rows(4);
        bad[3].1 = "other".into();
        assert!(check_ledger(&bad).is_err());
    }
}
