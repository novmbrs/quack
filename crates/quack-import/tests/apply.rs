//! End-to-end: a ledger-5 Go database imported into a fresh PostgreSQL,
//! checked row by row (see `common` for the env vars).

mod common;

use anyhow::Result;
use common::{A, B, Scratch, id, v5};
use quack_import::GuildOutcome;

#[tokio::test(flavor = "multi_thread")]
async fn apply_maps_every_table_and_a_rerun_is_a_no_op() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    let report = s.import(false).await?;
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.guilds[A], GuildOutcome::Imported);
    assert_eq!(report.guilds[B], GuildOutcome::Imported);
    // The report counts what PostgreSQL holds.
    for (table, rows) in s.counts().await? {
        if table == "discord_accounts" || table == "import_guilds" {
            continue;
        }
        assert_eq!(
            report.written_total(&table),
            u64::try_from(rows)?,
            "{table}"
        );
    }
    servers_and_settings(&s).await?;
    rules(&s).await?;
    cases(&s).await?;
    actions(&s).await?;
    dms_and_messages(&s).await?;
    evidence(&s).await?;
    appeals(&s).await?;
    modules(&s).await?;
    mod_log(&s).await?;
    report_sections(&report);

    let before = s.counts().await?;
    let again = s.import(false).await?;
    assert!(again.ok, "{}", again.summary());
    assert_eq!(again.guilds[A], GuildOutcome::AlreadyImported);
    assert_eq!(again.guilds[B], GuildOutcome::AlreadyImported);
    assert_eq!(s.counts().await?, before, "a rerun wrote rows");
    assert_eq!(
        s.count(
            "SELECT count(*) FROM import_runs WHERE mode = 'apply' AND finished_at IS NOT NULL"
        )
        .await?,
        2
    );
    s.drop().await
}

async fn servers_and_settings(s: &Scratch) -> Result<()> {
    assert_eq!(
        s.text(&format!(
            "SELECT icon_hash FROM guilds WHERE guild_id = {A}"
        ))
        .await?
        .as_deref(),
        Some("a_abc123")
    );
    assert_eq!(
        s.count(&format!(
            "SELECT count(*) FROM guilds WHERE guild_id = {B} AND NOT installed"
        ))
        .await?,
        1
    );
    assert_eq!(
        s.count(&format!(
            "SELECT last_number FROM case_counters WHERE guild_id = {A}"
        ))
        .await?,
        9_007_199_254_740_991
    );
    assert_eq!(
        s.count(&format!(
            "SELECT last_number FROM case_counters WHERE guild_id = {B}"
        ))
        .await?,
        1
    );
    let settings = s
        .text(&format!(
            "SELECT concat_ws('|', log_channel_id, appeal_channel_id, coalesce(evidence_channel_id::text, '-'), appeal_invite_url,
                     dm_intro, coalesce(dm_footer, '-'), starter_rule_id, array_to_string(moderator_role_ids, ','),
                     cardinality(rules_manager_role_ids))
             FROM guild_settings WHERE guild_id = {A}"
        ))
        .await?;
    assert_eq!(
        settings.as_deref(),
        Some(
            format!(
                "400000000000000001|400000000000000002|-|https://discord.gg/abcd|Hello|-|{}|600000000000000001,600000000000000002|0",
                id("TP", 1)
            )
            .as_str()
        )
    );
    assert_eq!(s.count("SELECT count(*) FROM discord_accounts").await?, 2);
    Ok(())
}

async fn rules(s: &Scratch) -> Result<()> {
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM rules WHERE guild_id = {A}"))
            .await?,
        10
    );
    // "spam" clashes with "Spam"; "Spam (2)" is taken by an archived rule.
    assert_eq!(
        s.text(&format!(
            "SELECT name FROM rules WHERE id = '{}'",
            id("TP", 3)
        ))
        .await?
        .as_deref(),
        Some("spam (3)")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT definition->>'name' FROM rule_versions WHERE rule_id = '{}'",
            id("TP", 3)
        ))
        .await?
        .as_deref(),
        Some("spam (3)")
    );
    // A level at case count 1 replaced the default level.
    assert_eq!(
        s.text(&format!("SELECT definition->'levels'->0->'action'->>'kind' FROM rule_versions WHERE rule_id = '{}'", id("TP", 5)))
            .await?
            .as_deref(),
        Some("kick")
    );
    // Harassment v3 is current; v1 and v2 were rebuilt from case snapshots.
    let versions = s
        .text(&format!(
            "SELECT string_agg(version || ':' || reconstructed || ':' || jsonb_array_length(definition->'levels'), ',' ORDER BY version)
             FROM rule_versions WHERE rule_id = '{}'",
            id("TP", 2)
        ))
        .await?;
    assert_eq!(versions.as_deref(), Some("1:true:2,2:true:1,3:false:2"));
    // The earliest case's snapshot wins.
    assert_eq!(
        s.text(&format!(
            "SELECT definition->>'name' FROM rule_versions WHERE rule_id = '{}' AND version = 1",
            id("TP", 2)
        ))
        .await?
        .as_deref(),
        Some("Harassment (old)")
    );
    // A template that is gone is rebuilt as an archived rule; one of another
    // server is copied under a derived ID.
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM rules WHERE id = '{}' AND archived_at IS NOT NULL AND current_version = 4", id("TP", 99)))
            .await?,
        1
    );
    assert_eq!(s.count(&format!("SELECT count(*) FROM rules WHERE guild_id = {A} AND name = 'Spam' AND archived_at IS NOT NULL")).await?, 1);
    Ok(())
}

async fn cases(s: &Scratch) -> Result<()> {
    let case = |n: u32, col: &str| format!("SELECT {col} FROM cases WHERE id = '{}'", id("CS", n));
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM cases WHERE guild_id = {A}"))
            .await?,
        20
    );
    assert_eq!(
        s.count(&case(14, "count(*)")).await?,
        0,
        "a case with an invalid member is skipped"
    );
    assert_eq!(
        s.text(&case(1, "context::text")).await?.as_deref(),
        Some(
            r#"{"note": "context note", "where": "https://discord.com/channels/100000000000000001/400000000000000003/500000000000000001"}"#
        )
    );
    assert_eq!(
        s.text(&case(4, "context->>'count'")).await?.as_deref(),
        Some("2.5")
    );
    assert_eq!(
        s.text(&case(1, "operation_key")).await?.as_deref(),
        Some("legacy:discord:111")
    );
    assert_eq!(
        s.text(&case(2, "concat_ws('|', action_kind, timeout_seconds, level_index, level_from_case_count, case_count, sends_dm, source)"))
            .await?
            .as_deref(),
        Some("timeout|600|2|3|3|t|dashboard")
    );
    assert_eq!(
        s.text(&case(
            5,
            "concat_ws('|', void_reason, void_cause, voided_by)"
        ))
        .await?
        .as_deref(),
        Some("(no reason recorded)|appeal_accepted|300000000000000002")
    );
    assert_eq!(
        s.text(&case(15, "void_cause::text")).await?.as_deref(),
        Some("staff")
    );
    assert_eq!(
        s.text(&case(8, "replaces_case_id::text")).await?,
        Some(id("CS", 5))
    );
    assert_eq!(
        s.text(&case(9, "replaces_case_id::text")).await?,
        None,
        "the second replacement is dropped"
    );
    assert_eq!(
        s.text(&case(10, "concat_ws('|', source, rule_id, sends_dm, imported->>'v4_action', imported->>'v4_moderator', imported->>'v4_source_id', imported->>'v4_reason')"))
            .await?
            .as_deref(),
        Some("imported|f|ban|OldMod|abc|old reason")
    );
    assert_eq!(
        s.text(&case(
            11,
            "concat_ws('|', source, coalesce(moderator_id::text, '-'), ban_delete_message_seconds)"
        ))
        .await?
        .as_deref(),
        Some("honeypot|-|0")
    );
    assert_eq!(s.count(&case(20, "number")).await?, 9_007_199_254_740_991);
    Ok(())
}

async fn actions(s: &Scratch) -> Result<()> {
    let action = |n: u32| {
        format!(
            "SELECT concat_ws('|', kind, state, coalesce(failure_reason::text, '-'), coalesce(failure_http_status::text, '-'),
                              coalesce(skip_reason::text, '-'), no_change, dismissed_at IS NOT NULL)
             FROM actions WHERE id = '{}'",
            id("EX", n)
        )
    };
    for (n, want) in [
        (1, "timeout|done|-|-|-|f|f"),
        (3, "remove_timeout|done|-|-|-|t|f"),
        (4, "ban|failed|unconfirmed|-|-|f|f"),
        (5, "timeout|pending|-|-|-|f|f"),
        (6, "timeout|pending|-|-|-|f|f"),
        (7, "kick|failed|discord_refused|403|-|f|t"),
        (8, "ban|done|-|-|-|f|f"),
        (9, "unban|skipped|-|-|changed_outside_case|f|f"),
        (10, "timeout|failed|unconfirmed|-|-|f|f"),
        (12, "timeout|cancelled|-|-|-|f|f"),
        (13, "ban|done|-|-|-|f|f"),
        (14, "unban|pending|-|-|-|f|f"),
    ] {
        assert_eq!(
            s.text(&action(n)).await?.as_deref(),
            Some(want),
            "execution {n}"
        );
    }
    for skipped in [2, 11, 15] {
        assert_eq!(
            s.count(&format!(
                "SELECT count(*) FROM actions WHERE id = '{}'",
                id("EX", skipped)
            ))
            .await?,
            0
        );
    }
    assert_eq!(
        s.text(&format!("SELECT to_char(timeout_ends_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM actions WHERE id = '{}'", id("EX", 1)))
            .await?
            .as_deref(),
        Some("2026-01-01 02:00:00")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT outcome::text FROM action_attempts WHERE action_id = '{}'",
            id("EX", 4)
        ))
        .await?
        .as_deref(),
        Some("interrupted")
    );
    // Only never-attempted work runs after cutover.
    let jobs = s
        .text(
            "SELECT string_agg(a.id::text, ',' ORDER BY a.id) FROM jobs j JOIN actions a ON a.id = (j.payload->>'action_id')::uuid
             WHERE j.kind = 'run_action'",
        )
        .await?;
    let mut want = [id("EX", 5), id("EX", 6), id("EX", 14)];
    want.sort();
    assert_eq!(jobs, Some(want.join(",")));
    Ok(())
}

async fn dms_and_messages(s: &Scratch) -> Result<()> {
    let dm = |n: u32| {
        format!(
            "SELECT concat_ws('|', state, coalesce(failure::text, '-')) FROM dms WHERE case_id = '{}' AND kind = 'case'",
            id("CS", n)
        )
    };
    for (n, want) in [
        (1, "sent|-"),
        (2, "unknown|-"),
        (3, "unknown|-"),
        (4, "pending|-"),
        (5, "not_needed|-"),
        (6, "failed|dms_closed"),
        (7, "not_needed|-"),
        (8, "pending|-"),
        (12, "failed|discord_unavailable"),
        (13, "unknown|-"),
    ] {
        assert_eq!(s.text(&dm(n)).await?.as_deref(), Some(want), "case {n}");
    }
    assert_eq!(
        s.count("SELECT count(*) FROM jobs WHERE kind = 'send_dm'")
            .await?,
        3
    );
    assert_eq!(
        s.count("SELECT count(*) FROM dms WHERE state = 'pending'")
            .await?,
        3
    );
    let receipts = s
        .text(&format!("SELECT concat_ws('|', message_id, wanted_revision, posted_revision) FROM messages WHERE case_id = '{}'", id("CS", 1)))
        .await?;
    assert_eq!(receipts.as_deref(), Some("610000000000000002|1|1"));
    // Open appeals' posts and the module messages are re-rendered.
    assert_eq!(
        s.count("SELECT count(*) FROM jobs WHERE kind = 'sync_message'")
            .await?,
        4
    );
    assert_eq!(
        s.count("SELECT count(*) FROM messages WHERE wanted_revision > posted_revision AND channel_id IS NOT NULL").await?,
        4
    );
    // A decided appeal with no post waits for a channel and gets no job.
    assert_eq!(
        s.count("SELECT count(*) FROM messages WHERE channel_id IS NULL")
            .await?,
        1
    );
    assert_eq!(
        s.count("SELECT count(*) FROM jobs WHERE kind = 'honeypot_cleanup'")
            .await?,
        1
    );
    Ok(())
}

async fn evidence(s: &Scratch) -> Result<()> {
    let e1 = id("SN", 1);
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', state, char_length(content), jsonb_array_length(embeds), files_not_saved) FROM evidence WHERE id = '{e1}'"))
            .await?
            .as_deref(),
        Some("saved|4000|10|0")
    );
    // The duplicate snapshot's new file is merged; its repeat of a file is not.
    assert_eq!(
        s.text(&format!("SELECT string_agg(copy_state::text, ',' ORDER BY position) FROM evidence_files WHERE evidence_id = '{e1}'"))
            .await?
            .as_deref(),
        Some("copied,link_only,link_only,link_only")
    );
    assert_eq!(
        s.count(&format!(
            "SELECT files_not_saved FROM evidence WHERE id = '{}'",
            id("SN", 3)
        ))
        .await?,
        1
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', state, failure, message_created_at IS NULL) FROM evidence WHERE id = '{}'", id("SN", 5)))
            .await?
            .as_deref(),
        Some("failed|discord_error|t")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT content FROM evidence WHERE id = '{}'",
            id("SN", 6)
        ))
        .await?
        .as_deref(),
        Some("ctrl\u{FFFD}char")
    );
    // An uploaded file is evidence of its copy in the evidence channel.
    assert_eq!(
        s.text(&format!(
            "SELECT concat_ws('|', e.state, e.channel_id, e.message_id, f.copy_state, f.copy_attachment_id, f.attachment_id)
             FROM evidence e JOIN evidence_files f ON f.evidence_id = e.id WHERE e.id = '{}'",
            id("SN", 9)
        ))
        .await?
        .as_deref(),
        Some("saved|400000000000000005|910000000000000009|copied|920000000000000009|940000000000000001")
    );
    Ok(())
}

async fn appeals(s: &Scratch) -> Result<()> {
    assert_eq!(s.count("SELECT count(*) FROM appeals").await?, 5);
    assert_eq!(
        s.text(&format!(
            "SELECT statement FROM appeals WHERE id = '{}'",
            id("AP", 7)
        ))
        .await?
        .as_deref(),
        Some("(no text)")
    );
    assert_eq!(
        s.count(&format!(
            "SELECT char_length(statement) FROM appeals WHERE id = '{}'",
            id("AP", 6)
        ))
        .await?,
        4100
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', closed_by_void, actor_id IS NULL) FROM appeal_events WHERE id = '{}'", id("AE", 10)))
            .await?
            .as_deref(),
        Some("t|t")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT text FROM appeal_events WHERE id = '{}'",
            id("AE", 6)
        ))
        .await?
        .as_deref(),
        Some("(no text)")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT kind::text FROM appeal_events WHERE id = '{}'",
            id("AE", 6)
        ))
        .await?
        .as_deref(),
        Some("information_provided")
    );
    assert!(
        s.text(&format!(
            "SELECT first_decided_at::text FROM appeals WHERE id = '{}'",
            id("AP", 1)
        ))
        .await?
        .is_some()
    );
    let post = |n: u32| {
        format!(
            "SELECT concat_ws('|', coalesce(channel_id::text, '-'), coalesce(message_id::text, '-'), wanted_revision, posted_revision)
             FROM messages WHERE appeal_id = '{}'",
            id("AP", n)
        )
    };
    assert_eq!(
        s.text(&post(1)).await?.as_deref(),
        Some("400000000000000002|720000000000000001|1|1")
    );
    assert_eq!(
        s.text(&post(2)).await?.as_deref(),
        Some("400000000000000002|720000000000000002|2|1")
    );
    assert_eq!(
        s.text(&post(3)).await?.as_deref(),
        Some("400000000000000002|-|1|0")
    );
    assert_eq!(
        s.text(&post(6)).await?.as_deref(),
        Some("400000000000000002|720000000000000006|1|1")
    );
    assert_eq!(s.text(&post(7)).await?.as_deref(), Some("-|-|1|0"));
    let appeal_dms = s.text("SELECT string_agg(state::text, ',' ORDER BY state) FROM dms WHERE kind = 'appeal_update'").await?;
    assert_eq!(appeal_dms.as_deref(), Some("pending,sent,unknown"));
    Ok(())
}

async fn modules(s: &Scratch) -> Result<()> {
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', enabled, entry_channel_id, queue_channel_id, transcript_retention_days) FROM ticket_settings WHERE guild_id = {A}"))
            .await?
            .as_deref(),
        Some("t|400000000000000007|400000000000000008|30")
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', enabled, transcript_retention_days) FROM ticket_settings WHERE guild_id = {B}")).await?.as_deref(),
        Some("f|365")
    );
    assert_eq!(
        s.text(&format!("SELECT string_agg(event::text, ',' ORDER BY event) FROM activity_log_routes WHERE guild_id = {A}")).await?.as_deref(),
        Some("message_edited,member_joined")
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', enabled, include_message_content, include_file_names, include_embed_types) FROM activity_log_settings WHERE guild_id = {A}"))
            .await?
            .as_deref(),
        Some("t|f|t|f")
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', enabled, rule_id, coalesce(warning_text, '-')) FROM honeypot_settings WHERE guild_id = {A}"))
            .await?,
        Some(format!("t|{}|-", id("TP", 7)))
    );
    assert_eq!(
        s.text(&format!("SELECT concat_ws('|', enabled, disabled_reason) FROM honeypot_settings WHERE guild_id = {B}")).await?.as_deref(),
        Some("f|channel_deleted")
    );
    let ticket = |n: u32| {
        format!(
            "SELECT concat_ws('|', state, coalesce(closed_by::text, '-')) FROM tickets WHERE id = '{}'",
            id("TK", n)
        )
    };
    assert_eq!(
        s.text(&ticket(1)).await?.as_deref(),
        Some("closed|300000000000000002")
    );
    assert_eq!(
        s.text(&ticket(2)).await?.as_deref(),
        Some("closed|-"),
        "the older live ticket is closed"
    );
    assert_eq!(s.text(&ticket(3)).await?.as_deref(), Some("open|-"));
    assert_eq!(s.text(&ticket(4)).await?.as_deref(), Some("closed|-"));
    assert_eq!(
        s.text("SELECT string_agg(state::text || coalesce(':' || failure::text, ''), ',' ORDER BY state) FROM dms WHERE kind = 'ticket_closed'")
            .await?
            .as_deref(),
        Some("failed:discord_refused,unknown")
    );
    assert_eq!(
        s.count("SELECT count(*) FROM ticket_messages WHERE author_name = 'unknown'")
            .await?,
        1
    );
    assert_eq!(s.count("SELECT count(*) FROM ticket_transcripts").await?, 1);
    let hits = s.text("SELECT string_agg(state::text || ':' || coalesce(failure, '-'), ',' ORDER BY message_id) FROM honeypot_hits").await?;
    assert_eq!(
        hits.as_deref(),
        Some(
            "failed:imported_pending,failed:exempt,failed:case_application_failed,case_created:-,case_created:-"
        )
    );
    Ok(())
}

async fn mod_log(s: &Scratch) -> Result<()> {
    assert_eq!(
        s.count("SELECT count(*) FROM mod_log WHERE kind = 'legacy' AND source = 'imported'")
            .await?,
        9
    );
    let entry =
        |n: u32, cols: &str| format!("SELECT {cols} FROM mod_log WHERE id = '{}'", id("AD", n));
    assert_eq!(
        s.text(&entry(1, "concat_ws('|', case_id, member_id)"))
            .await?,
        Some(format!("{}|200000000000000001", id("CS", 1)))
    );
    assert_eq!(
        s.text(&entry(
            2,
            "concat_ws('|', coalesce(case_id::text, '-'), appeal_id)"
        ))
        .await?,
        Some(format!("-|{}", id("AP", 2)))
    );
    assert_eq!(
        s.text(&entry(3, "action_id::text")).await?,
        Some(id("EX", 1))
    );
    assert_eq!(s.text(&entry(4, "rule_id::text")).await?, Some(id("TP", 1)));
    assert_eq!(
        s.text(&entry(5, "ticket_id::text")).await?,
        Some(id("TK", 1))
    );
    assert_eq!(
        s.text(&entry(
            6,
            "concat_ws('|', member_id, coalesce(actor_id::text, '-'))"
        ))
        .await?
        .as_deref(),
        Some("200000000000000003|-")
    );
    assert_eq!(
        s.text(&entry(
            7,
            "concat_ws('|', detail->>'failure_reason', coalesce(detail->>'resource_id', '-'))"
        ))
        .await?
        .as_deref(),
        Some("[REDACTED]|-")
    );
    assert_eq!(
        s.text(&entry(
            8,
            "concat_ws('|', result, coalesce(case_id::text, '-'))"
        ))
        .await?
        .as_deref(),
        Some("refused|-")
    );
    assert_eq!(
        s.text(&entry(10, "coalesce(member_id::text, '-')"))
            .await?
            .as_deref(),
        Some("-")
    );
    assert_eq!(
        s.count("SELECT count(*) FROM jobs WHERE kind = 'post_mod_log'")
            .await?,
        0,
        "imported entries are never posted"
    );
    Ok(())
}

fn report_sections(r: &quack_import::Report) {
    let skipped = |table: &str, reason: &str| {
        r.skipped
            .get(table)
            .and_then(|m| m.get(reason))
            .map_or(0, |e| e.count)
    };
    assert_eq!(skipped("cases", "invalid_snowflake"), 1);
    assert_eq!(skipped("case_action_executions", "dm_action_row"), 1);
    assert_eq!(skipped("case_action_executions", "duplicate_undo"), 1);
    assert_eq!(skipped("appeals", "null_case_appeal"), 1);
    assert_eq!(skipped("appeals", "member_mismatch"), 1);
    assert_eq!(skipped("ticket_transcripts", "expired_transcript"), 1);
    assert_eq!(skipped("case_evidence_snapshots", "duplicate_evidence"), 1);
    assert_eq!(skipped("case_evidence_attachments", "extra_attachment"), 1);
    assert_eq!(skipped("cases.replaces_case_id", "invalid_replacement"), 1);
    assert_eq!(skipped("tickets", "duplicate_live_ticket"), 1);
    assert_eq!(skipped("activity_log_routes", "invalid_route"), 2);
    assert_eq!(skipped("case_notifications", "missing_parent"), 2);
    assert_eq!(skipped("guild_settings", "invalid_snowflake"), 1);
    assert_eq!(skipped("discord_user_mfa", "invalid_snowflake"), 1);
    assert_eq!(r.renamed_rules.len(), 1);
    assert_eq!(r.reconstructed_versions.len(), 4);
    assert_eq!(r.conflicting_snapshots.count, 1);
    assert_eq!(r.dropped_default_levels.count, 1);
    assert_eq!(r.in_flight["pending_action_resumed"].count, 3);
    assert_eq!(r.in_flight["running_action_unconfirmed"].count, 1);
    assert_eq!(r.in_flight["dm_sending_unknown"].count, 1);
    assert_eq!(r.in_flight["honeypot_pending_failed"].count, 1);
    assert_eq!(r.in_flight["mirror_delivery_dropped"].count, 1);
    assert_eq!(r.derived["unconfirmed_dm"].count, 2);
    assert_eq!(r.derived["uploaded_file_as_copy_message"].count, 1);
    assert_eq!(r.in_flight["appeal_staff_post_never_delivered"].count, 1);
    assert_eq!(r.over_limit["evidence.content"].count, 1);
    assert_eq!(r.legacy_mod_log.values().sum::<u64>(), 9);
    assert_eq!(
        r.not_imported["case_events"]
            .values()
            .map(|e| e.count)
            .sum::<u64>(),
        3
    );
    assert!(r.checks.values().all(|c| c.ok));
    assert!(
        r.source
            .manifest
            .iter()
            .any(|m| m.table == "cases" && m.column == "reason")
    );
    assert!(
        r.source
            .shape
            .contains(&"table discord_user_mfa".to_owned())
    );
    assert!(r.summary().contains("OK"));
}
