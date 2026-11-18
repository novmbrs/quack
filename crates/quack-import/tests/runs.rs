//! End-to-end runs: dry run, resuming after a failure or an interruption,
//! and the refusals (see `common` for the env vars).

mod common;

use anyhow::Result;
use common::{
    A, B, BROKEN, DATA, LEDGER_V3, LEGACY_APPEALS, SCHEMA_V3, Scratch, UPGRADE_V5, id, ulid, v5,
};
use quack_import::{GuildOutcome, ImportError};

#[tokio::test(flavor = "multi_thread")]
async fn dry_run_checks_everything_and_writes_nothing() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    let dry = s.import(true).await?;
    assert!(dry.ok, "{}", dry.summary());
    assert_eq!(dry.guilds[A], GuildOutcome::Checked);
    for (table, rows) in s.counts().await? {
        assert_eq!(rows, 0, "a dry run wrote {table}");
    }
    assert_eq!(
        s.count("SELECT count(*) FROM import_runs WHERE mode = 'dry_run' AND report IS NOT NULL")
            .await?,
        1
    );
    let real = s.import(false).await?;
    assert_eq!(
        dry.written, real.written,
        "a dry run reports what the import writes"
    );
    assert_eq!(dry.skipped, real.skipped);
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn ledger_three_with_legacy_appeal_columns() -> Result<()> {
    let Some(s) = Scratch::new(&[SCHEMA_V3, DATA, LEDGER_V3, LEGACY_APPEALS]).await? else {
        return Ok(());
    };
    let report = s.import(false).await?;
    assert!(report.ok, "{}", report.summary());
    assert_eq!(
        s.text(&format!(
            "SELECT statement FROM appeals WHERE id = '{}'",
            id("AP", 2)
        ))
        .await?
        .as_deref(),
        Some("first answer\n\nsecond answer")
    );
    assert_eq!(
        s.text(&format!(
            "SELECT statement FROM appeals WHERE id = '{}'",
            id("AP", 1)
        ))
        .await?
        .as_deref(),
        Some("please")
    );
    assert_eq!(s.count("SELECT count(*) FROM discord_accounts").await?, 0);
    assert_eq!(
        s.text(&format!(
            "SELECT cardinality(moderator_role_ids)::text FROM guild_settings WHERE guild_id = {A}"
        ))
        .await?
        .as_deref(),
        Some("0")
    );
    assert_eq!(
        report.in_flight["ticket_queue_send_unconfirmed_dropped"].count,
        1
    );
    assert_eq!(
        report.not_imported["guild_appeal_settings"]["rows"].count,
        1
    );
    assert!(
        report
            .source
            .shape
            .contains(&"appeals.answers_json".to_owned())
    );
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_server_rolls_back_and_a_rerun_completes_it() -> Result<()> {
    let Some(s) = Scratch::new(&[SCHEMA_V3, DATA, LEDGER_V3, UPGRADE_V5, BROKEN]).await? else {
        return Ok(());
    };
    // Plant a row server B's import collides with, as if a run died after
    // something else wrote it.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO guilds (guild_id, name, owner_id) VALUES ({B}, 'x', 1)"
    )))
    .execute(s.db.pool())
    .await?;
    let first = s.import(false).await?;
    assert!(!first.ok);
    assert_eq!(first.guilds[A], GuildOutcome::Imported);
    assert_eq!(first.guilds[B], GuildOutcome::Failed);
    assert!(
        first
            .problems
            .contains_key("guild:01JAGD00030000000000000000"),
        "{:?}",
        first.problems
    );
    assert!(!first.checks["source_cross_guild_links"].ok);
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM rules WHERE guild_id = {B}"))
            .await?,
        0,
        "B rolled back"
    );
    let a_rows = s
        .count(&format!("SELECT count(*) FROM cases WHERE guild_id = {A}"))
        .await?;

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM guilds WHERE guild_id = {B}"
    )))
    .execute(s.db.pool())
    .await?;
    let second = s.import(false).await?;
    assert_eq!(second.guilds[A], GuildOutcome::AlreadyImported);
    assert_eq!(second.guilds[B], GuildOutcome::Imported);
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM cases WHERE guild_id = {A}"))
            .await?,
        a_rows
    );
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM cases WHERE guild_id = {B}"))
            .await?,
        1
    );
    assert_eq!(s.count("SELECT count(*) FROM import_guilds").await?, 2);
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_run_leaves_nothing_half_done() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    // Stop the run as soon as the first server has committed.
    let db = s.db.clone();
    let source = s.source_url.clone();
    let task = tokio::spawn(async move {
        quack_import::run(
            &db,
            &quack_import::ImportOptions {
                source_url: source,
                dry_run: false,
                report_path: None,
            },
        )
        .await
    });
    loop {
        if s.count("SELECT count(*) FROM import_guilds").await? >= 1 || task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    task.abort();
    let _ = task.await;
    // A statement the aborted run already sent (a COMMIT, say) still runs
    // to its end on the server: wait until no other session of this
    // database is busy, so nothing lands while the checks below read.
    let busy = "SELECT count(*) FROM pg_stat_activity
                WHERE datname = current_database() AND pid <> pg_backend_pid()
                  AND state <> 'idle'";
    let quiet = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while s.count(busy).await? > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        anyhow::Ok(())
    })
    .await;
    assert!(quiet.is_ok(), "the aborted run's session never went idle");
    // Whatever committed is whole: every server with rows has its
    // import_guilds row. One statement, so one snapshot.
    let partial = s
        .count(
            "SELECT count(*) FROM (SELECT DISTINCT guild_id FROM cases) c
             WHERE NOT EXISTS (SELECT 1 FROM import_guilds g WHERE g.guild_id = c.guild_id)",
        )
        .await?;
    assert_eq!(
        partial, 0,
        "a server has rows without its import_guilds row"
    );
    let report = s.import(false).await?;
    assert!(report.ok, "{}", report.summary());
    assert_eq!(s.count("SELECT count(*) FROM import_guilds").await?, 2);
    assert_eq!(s.count("SELECT count(*) FROM cases").await?, 21);
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_source_data_is_refused() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    assert!(s.import(false).await?.ok);
    s.mysql(&format!(
        "UPDATE cases SET reason = 'changed' WHERE id = '{}'",
        ulid("CS", 1)
    ))
    .await?;
    match s.import(false).await {
        Err(ImportError::ChangedSource { .. }) => {}
        other => panic!("expected a refusal, got {other:?}"),
    }
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_shapes_and_uuid_collisions_are_refused() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    s.mysql("ALTER TABLE cases ADD COLUMN surprise int NULL")
        .await?;
    match s.import(true).await {
        Err(ImportError::Shape(problems)) => {
            assert_eq!(problems, ["unexpected column cases.surprise"]);
        }
        other => panic!("expected a shape refusal, got {other:?}"),
    }
    s.mysql("ALTER TABLE cases DROP COLUMN surprise").await?;
    // Two IDs that differ only in the 6 bits the mapping drops.
    s.mysql("INSERT INTO staff_members (id, created_at, updated_at, guild_id, discord_user_id) VALUES ('01JASF00010000000000000002', NOW(), NOW(), 'x', 'y')").await?;
    match s.import(true).await {
        Err(ImportError::Collision(keys)) => assert_ne!(keys, [] as [std::string::String; 0]),
        other => panic!("expected a collision refusal, got {other:?}"),
    }
    assert_eq!(
        s.count("SELECT count(*) FROM import_runs").await?,
        0,
        "nothing was written"
    );
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_quack_holds_the_fence() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    let fence = quack_db::Fence::try_acquire(s.db.connect_options())
        .await?
        .expect("the fence is free");
    assert!(matches!(s.import(true).await, Err(ImportError::Fenced)));
    fence.release().await?;
    assert!(s.import(true).await?.ok);
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_new_limits_refuse_are_quarantined_or_stop_their_server() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    // A transcript over the 8 MiB the new schema holds is quarantined.
    s.mysql(&format!(
        "INSERT INTO ticket_transcripts (ticket_id, guild_id, content, captured_at, expires_at)
         VALUES ('{}', '{}', REPEAT('x', 8400000), '2026-01-02 00:00:00', '2099-01-01 00:00:00')",
        ulid("TK", 3),
        ulid("GD", 1)
    ))
    .await?;
    // A case number above 2^53 - 1 stops its server (§19.3).
    s.mysql(&format!(
        "UPDATE cases SET case_number = 9007199254740992 WHERE id = '{}'",
        ulid("CS", 21)
    ))
    .await?;
    let report = s.import(false).await?;
    assert!(!report.ok);
    assert_eq!(report.guilds[A], GuildOutcome::Imported);
    assert_eq!(report.guilds[B], GuildOutcome::Failed);
    assert!(
        report.problems[B]
            .iter()
            .any(|p| p.contains("case number 9007199254740992 is out of range")),
        "{:?}",
        report.problems
    );
    let quarantined = &report.quarantined["ticket_transcripts"];
    assert_eq!(
        quarantined.values().map(|e| e.count).sum::<u64>(),
        1,
        "{quarantined:?}"
    );
    assert_eq!(s.count("SELECT count(*) FROM ticket_transcripts").await?, 1);
    assert_eq!(
        s.count(&format!("SELECT count(*) FROM guilds WHERE guild_id = {B}"))
            .await?,
        0
    );
    s.drop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_case_with_an_invalid_moderator_is_written_without_failing_its_server() -> Result<()> {
    let Some(s) = v5().await? else { return Ok(()) };
    s.mysql(&format!(
        "UPDATE cases SET moderator_discord_user_id = 'not-a-snowflake' WHERE id = '{}'",
        ulid("CS", 21)
    ))
    .await?;
    let report = s.import(false).await?;
    assert!(report.ok, "{}", report.summary());
    assert_eq!(report.guilds[B], GuildOutcome::Imported);
    assert_eq!(report.derived["case_moderator_invalid"].count, 1);
    assert!(report.checks["every_case_accounted_for"].ok);
    s.drop().await
}
