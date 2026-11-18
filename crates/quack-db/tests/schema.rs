//! The migration and the constraints and triggers the design relies on.

mod common;

use anyhow::Result;
use common::{account_id, case, db, guild, member_id, rule, sqlstate};
use quack_core::action::{ActionKind, AttemptOutcome};
use quack_core::ids::{ActionId, AppealId, ChannelId, DmId, EvidenceId, TicketId};
use quack_core::modlog::{ModLogEntry, ModLogLinks, ModLogSource, RuleVersionDetail};
use quack_core::rule::LevelAction;
use quack_db::actions::{self, NewAction};
use quack_db::dms::{self, DmOwner};
use quack_db::modlog::{self, NewEntry};
use quack_db::operation_keys::{self, OperationKey};
use quack_db::{
    Conflict, DbError, MigrationMismatch, Violation, appeals, evidence, guilds, tickets,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

const TIMEOUT: Option<LevelAction> = Some(LevelAction::Timeout {
    duration_seconds: 600,
});

#[sqlx::test(migrations = false)]
async fn the_migration_file_applies_in_one_transaction(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let mut tx = db.begin().await?;
    sqlx::raw_sql(include_str!("../migrations/0001_initial.sql"))
        .execute(&mut *tx)
        .await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    assert_eq!(tables, 31);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn migrate_then_verify_detects_every_mismatch(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    assert_eq!(
        quack_db::verify_migrations(&db).await?,
        Err(MigrationMismatch::NotMigrated)
    );
    let applied = quack_db::migrate(&db).await?;
    assert_eq!(applied.first().map(|m| m.version), Some(1));
    assert_eq!(quack_db::verify_migrations(&db).await?, Ok(()));
    // Running it again is a no-op.
    quack_db::migrate(&db).await?;
    assert_eq!(quack_db::verify_migrations(&db).await?, Ok(()));

    let mut conn = db.conn().await?;
    sqlx::query("UPDATE _sqlx_migrations SET checksum = '\\x00' WHERE version = 1")
        .execute(&mut *conn)
        .await?;
    assert_eq!(
        quack_db::verify_migrations(&db).await?,
        Err(MigrationMismatch::Changed(1))
    );
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (99, 'future', true, '\\x00', 0)",
    )
    .execute(&mut *conn)
    .await?;
    assert_eq!(
        quack_db::verify_migrations(&db).await?,
        Err(MigrationMismatch::Unknown(vec![99]))
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn the_moderation_log_and_rule_versions_are_append_only(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let entry = ModLogEntry::RuleCreated(RuleVersionDetail {
        name: "Spam".to_owned(),
        version: 1,
    });
    let mut tx = db.begin().await?;
    let written = modlog::insert(
        &mut tx,
        &NewEntry {
            guild: g,
            entry: &entry,
            actor: Some(account_id(5)),
            source: ModLogSource::Dashboard,
            links: ModLogLinks {
                rule_id: Some(rule_id),
                ..ModLogLinks::default()
            },
            legacy_result: None,
        },
    )
    .await?;
    tx.commit().await?;
    let mut conn = db.conn().await?;
    let stored = modlog::get(&mut conn, g, written.id).await?;
    assert_eq!(stored.map(|e| e.entry), Some(entry));

    for statement in [
        "UPDATE mod_log SET actor_id = NULL",
        "DELETE FROM mod_log",
        "TRUNCATE mod_log",
        "UPDATE rule_versions SET reconstructed = true",
        "DELETE FROM rule_versions",
        // Referenced by rules, so only a cascading truncate reaches the trigger.
        "TRUNCATE rule_versions CASCADE",
    ] {
        let error = sqlx::query(statement).execute(&mut *conn).await.err();
        let state = error.as_ref().and_then(sqlstate);
        assert_eq!(state.as_deref(), Some("42501"), "{statement}");
        assert!(matches!(
            error.map(DbError::from),
            Some(DbError::Rejected {
                violation: Violation::AppendOnly,
                ..
            })
        ));
    }
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn an_action_has_one_open_attempt_one_original_and_one_undo(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), rule_id, TIMEOUT).await?;
    let action = ActionId::from_uuid(Uuid::now_v7());
    let new = NewAction {
        id: action,
        guild: g,
        case: c.id,
        member: c.member,
        kind: ActionKind::Timeout,
        undo_of: None,
        timeout_ends_at: Some(c.created_at + time::Duration::minutes(10)),
        delete_message_seconds: None,
        requested_by: None,
        created_at: Some(c.created_at),
    };
    let mut conn = db.conn().await?;
    actions::insert(&mut conn, &new).await?;
    let second = actions::insert(
        &mut conn,
        &NewAction {
            id: ActionId::from_uuid(Uuid::now_v7()),
            ..new
        },
    )
    .await;
    assert_eq!(
        second.err().and_then(|e| e.conflict()),
        Some(Conflict::ActionExists)
    );

    assert_eq!(actions::start(&mut conn, g, action).await?, Some(1));
    assert_eq!(
        actions::start(&mut conn, g, action).await?,
        None,
        "already running"
    );
    let open = sqlx::query("INSERT INTO action_attempts (guild_id, action_id, number, started_at) VALUES ($1, $2, 2, now())")
        .bind(i64::from(g))
        .bind(action.get())
        .execute(&mut *conn)
        .await
        .err()
        .map(DbError::from);
    assert_eq!(open.and_then(|e| e.conflict()), Some(Conflict::AttemptOpen));
    assert!(
        actions::finish_attempt(
            &mut conn,
            g,
            action,
            1,
            AttemptOutcome::Done,
            Some(200),
            None
        )
        .await?
    );
    assert!(
        !actions::finish_attempt(&mut conn, g, action, 1, AttemptOutcome::Done, None, None).await?
    );
    assert!(actions::mark_done(&mut conn, g, action, false).await?);

    let undo = NewAction {
        id: ActionId::from_uuid(Uuid::now_v7()),
        kind: ActionKind::RemoveTimeout,
        undo_of: Some(action),
        timeout_ends_at: None,
        created_at: None,
        ..new
    };
    actions::insert(&mut conn, &undo).await?;
    let again = actions::insert(
        &mut conn,
        &NewAction {
            id: ActionId::from_uuid(Uuid::now_v7()),
            ..undo
        },
    )
    .await;
    assert_eq!(
        again.err().and_then(|e| e.conflict()),
        Some(Conflict::ActionExists)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_member_has_one_live_ticket(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let entry = ChannelId::new(50).unwrap_or_else(|| unreachable!());
    let mut conn = db.conn().await?;
    let first = TicketId::from_uuid(Uuid::now_v7());
    tickets::insert(&mut conn, first, g, member_id(7), entry).await?;
    let second = tickets::insert(
        &mut conn,
        TicketId::from_uuid(Uuid::now_v7()),
        g,
        member_id(7),
        entry,
    )
    .await;
    assert_eq!(
        second.err().and_then(|e| e.conflict()),
        Some(Conflict::LiveTicketExists)
    );
    // Another member is fine; closing frees the slot.
    tickets::insert(
        &mut conn,
        TicketId::from_uuid(Uuid::now_v7()),
        g,
        member_id(8),
        entry,
    )
    .await?;
    assert_eq!(
        tickets::fail_open(&mut conn, g, first).await?,
        Some(quack_core::modules::TicketState::Closed)
    );
    tickets::insert(
        &mut conn,
        TicketId::from_uuid(Uuid::now_v7()),
        g,
        member_id(7),
        entry,
    )
    .await?;
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn operation_keys_are_unique_per_scope(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let key = OperationKey {
        guild: Some(g),
        account: Some(account_id(5)),
        operation: "case_add".to_owned(),
        key: "123".to_owned(),
    };
    let hash = [7; 32];
    let mut tx = db.begin().await?;
    assert!(operation_keys::reserve(&mut tx, &key, &hash).await?);
    operation_keys::complete(&mut tx, &key, 201, &serde_json::json!({ "case_id": "x" })).await?;
    tx.commit().await?;

    let mut conn = db.conn().await?;
    assert!(!operation_keys::reserve(&mut conn, &key, &[8; 32]).await?);
    let other_account = OperationKey {
        account: Some(account_id(6)),
        ..key.clone()
    };
    assert!(operation_keys::reserve(&mut conn, &other_account, &hash).await?);
    let stored = operation_keys::lookup(&mut conn, &key).await?;
    assert_eq!(
        stored.map(|s| (s.request_hash, s.status)),
        Some((hash, 201))
    );
    let unscoped = OperationKey {
        guild: None,
        account: None,
        ..key.clone()
    };
    assert!(
        operation_keys::lookup(&mut conn, &unscoped)
            .await?
            .is_none()
    );
    assert_eq!(key.case_scoped(), "5:case_add:123");
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn children_must_match_their_parents_server_case_and_member(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let a = guild(&db, 1).await?;
    let b = guild(&db, 2).await?;
    let (rule_a, _) = rule(&db, a, "Spam").await?;
    let c = case(&db, a, member_id(7), rule_a, None).await?;
    let mut conn = db.conn().await?;

    let foreign_key = |result: Result<(), DbError>| {
        matches!(
            result,
            Err(DbError::Rejected {
                violation: Violation::ForeignKey,
                ..
            })
        )
    };
    // An action in another server, or about another member, than its case.
    let action = NewAction {
        id: ActionId::from_uuid(Uuid::now_v7()),
        guild: b,
        case: c.id,
        member: c.member,
        kind: ActionKind::Kick,
        undo_of: None,
        timeout_ends_at: None,
        delete_message_seconds: None,
        requested_by: None,
        created_at: None,
    };
    assert!(foreign_key(actions::insert(&mut conn, &action).await));
    let wrong_member = NewAction {
        guild: a,
        member: member_id(8),
        ..action
    };
    assert!(foreign_key(actions::insert(&mut conn, &wrong_member).await));
    // DMs and appeals likewise.
    let dm = dms::insert(
        &mut conn,
        DmId::from_uuid(Uuid::now_v7()),
        b,
        c.member,
        DmOwner::Case(c.id),
    )
    .await;
    assert!(foreign_key(dm));
    let dm = dms::insert(
        &mut conn,
        DmId::from_uuid(Uuid::now_v7()),
        a,
        member_id(8),
        DmOwner::Case(c.id),
    )
    .await;
    assert!(foreign_key(dm));
    let appeal = appeals::insert(
        &mut conn,
        AppealId::from_uuid(Uuid::now_v7()),
        a,
        c.id,
        member_id(8),
        "hi",
    )
    .await;
    assert!(foreign_key(appeal));
    // Evidence in another server than its case: refused before any write.
    let channel = ChannelId::new(9).unwrap_or_else(|| unreachable!());
    let message = quack_core::ids::MessageId::new(10).unwrap_or_else(|| unreachable!());
    let admitted = evidence::admit(
        &mut conn,
        b,
        c.id,
        EvidenceId::from_uuid(Uuid::now_v7()),
        channel,
        message,
        None,
    )
    .await;
    assert!(matches!(admitted, Err(DbError::NotFound)), "{admitted:?}");
    // A case under another server's rule.
    let mut tx = db.begin().await?;
    let other = common::insert_case(&mut tx, b, member_id(7), rule_a, None).await;
    assert!(matches!(
        other
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<DbError>()),
        Some(DbError::Rejected {
            violation: Violation::ForeignKey,
            ..
        })
    ));
    drop(tx);
    // The right parents work.
    dms::insert(
        &mut conn,
        DmId::from_uuid(Uuid::now_v7()),
        a,
        c.member,
        DmOwner::Case(c.id),
    )
    .await?;
    let again = dms::insert(
        &mut conn,
        DmId::from_uuid(Uuid::now_v7()),
        a,
        c.member,
        DmOwner::Case(c.id),
    )
    .await;
    assert_eq!(
        again.err().and_then(|e| e.conflict()),
        Some(Conflict::DmExists)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn case_numbers_stop_at_two_to_the_fifty_three(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let mut conn = db.conn().await?;
    sqlx::query("UPDATE case_counters SET last_number = 9007199254740990 WHERE guild_id = $1")
        .bind(i64::from(g))
        .execute(&mut *conn)
        .await?;
    assert_eq!(
        guilds::next_case_number(&mut conn, g).await?,
        9_007_199_254_740_991
    );
    let past = guilds::next_case_number(&mut conn, g).await;
    assert!(matches!(
        past,
        Err(DbError::Rejected {
            violation: Violation::Check,
            ..
        })
    ));
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn job_keys_must_start_with_their_kind(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let mut conn = db.conn().await?;
    let bad = sqlx::query("INSERT INTO jobs (kind, key, guild_id, max_attempts) VALUES ('send_dm', 'run_action:1', $1, 3)")
        .bind(i64::from(g))
        .execute(&mut *conn)
        .await
        .err();
    assert_eq!(bad.as_ref().and_then(sqlstate).as_deref(), Some("23514"));
    let no_guild = sqlx::query(
        "INSERT INTO jobs (kind, key, max_attempts) VALUES ('send_dm', 'send_dm:1', 3)",
    )
    .execute(&mut *conn)
    .await
    .err();
    assert_eq!(
        no_guild.as_ref().and_then(sqlstate).as_deref(),
        Some("23514")
    );
    Ok(())
}
