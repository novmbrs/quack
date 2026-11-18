//! Case creation under concurrency, locks, compare-and-set writes, and the
//! escalation count.

mod common;

use std::time::Duration;

use anyhow::{Context as _, Result};
use common::{HEAVY, account_id, case, db, db_with, guild, insert_case, member_id, rule};
use quack_core::case::{ContextValues, VoidCause};
use quack_db::{actions, cases};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::OffsetDateTime;

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn case_numbers_are_gapless_and_unique_under_50_concurrent_creators(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let _heavy = HEAVY.lock().await;
    let db = db_with(options, 52).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let mut tasks = Vec::new();
    for i in 0..60_u64 {
        let db = db.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = db.begin().await?;
            let id = insert_case(&mut tx, g, member_id(100 + i), rule_id, None).await?;
            // Every sixth creator fails after taking a number: its number
            // must not be consumed.
            if i % 6 == 5 {
                tx.rollback().await?;
                return Ok::<_, anyhow::Error>(None);
            }
            tx.commit().await?;
            Ok(Some(id))
        }));
    }
    let mut committed = Vec::new();
    for task in tasks {
        if let Some(id) = task.await?? {
            committed.push(id);
        }
    }
    assert_eq!(committed.len(), 50);
    let mut conn = db.conn().await?;
    let mut numbers: Vec<i64> =
        sqlx::query_scalar("SELECT number FROM cases WHERE guild_id = $1 ORDER BY number")
            .bind(i64::from(g))
            .fetch_all(&mut *conn)
            .await?;
    numbers.dedup();
    assert_eq!(numbers, (1..=50).collect::<Vec<i64>>());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn the_member_lock_serializes_action_starts(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let mut first = db.begin().await?;
    actions::lock_member(&mut first, g, member_id(7)).await?;

    // Another member is not blocked.
    let mut other = db.begin().await?;
    tokio::time::timeout(
        Duration::from_secs(2),
        actions::lock_member(&mut other, g, member_id(8)),
    )
    .await??;
    other.commit().await?;

    let waiter = {
        let db = db.clone();
        tokio::spawn(async move {
            let mut second = db.begin().await?;
            actions::lock_member(&mut second, g, member_id(7)).await?;
            let at = std::time::Instant::now();
            second.commit().await?;
            Ok::<_, anyhow::Error>(at)
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiter.is_finished(),
        "the second start must wait for the first"
    );
    let released = std::time::Instant::now();
    first.commit().await?;
    let acquired = tokio::time::timeout(Duration::from_secs(5), waiter).await???;
    assert!(acquired >= released);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_case_is_voided_exactly_once_under_a_race(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db_with(options, 12).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), rule_id, None).await?;
    let mut tasks = Vec::new();
    for i in 0..10_u64 {
        let db = db.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = db.begin().await?;
            let locked = cases::lock(&mut tx, g, c.id).await?.context("case")?;
            if !locked.is_valid() {
                return Ok::<_, anyhow::Error>(false);
            }
            let voided = cases::void(
                &mut tx,
                g,
                c.id,
                VoidCause::Staff,
                Some(account_id(10 + i)),
                "wrong",
            )
            .await?;
            tx.commit().await?;
            Ok(voided)
        }));
    }
    let mut wins = 0;
    for task in tasks {
        if task.await?? {
            wins += 1;
        }
    }
    assert_eq!(wins, 1);
    let mut conn = db.conn().await?;
    // A plain CAS without the lock also refuses.
    assert!(!cases::void(&mut conn, g, c.id, VoidCause::Staff, None, "again").await?);
    let stored = cases::get(&mut conn, g, c.id).await?.context("case")?;
    assert_eq!(
        stored.void.map(|v| (v.cause, v.reason)),
        Some((VoidCause::Staff, "wrong".to_owned()))
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn context_edits_compare_the_revision(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (rule_id, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), rule_id, None).await?;
    let mut conn = db.conn().await?;
    let values = ContextValues::default();
    assert_eq!(
        cases::set_context(&mut conn, g, c.id, 1, &values).await?,
        Some(2)
    );
    assert_eq!(
        cases::set_context(&mut conn, g, c.id, 1, &values).await?,
        None
    );
    assert_eq!(
        cases::set_context(&mut conn, g, c.id, 2, &values).await?,
        Some(3)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn escalation_counts_valid_cases_of_the_member_and_rule(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let (other_rule, _) = rule(&db, g, "Other").await?;
    let m = member_id(7);
    let first = case(&db, g, m, spam, None).await?;
    case(&db, g, m, spam, None).await?;
    case(&db, g, m, other_rule, None).await?;
    case(&db, g, member_id(8), spam, None).await?;
    let mut conn = db.conn().await?;
    assert_eq!(cases::count_counted(&mut conn, g, m, spam, None).await?, 2);
    cases::void(&mut conn, g, first.id, VoidCause::Staff, None, "wrong").await?;
    assert_eq!(cases::count_counted(&mut conn, g, m, spam, None).await?, 1);
    let future = OffsetDateTime::now_utc() + time::Duration::hours(1);
    assert_eq!(
        cases::count_counted(&mut conn, g, m, spam, Some(future)).await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn replacement_links_and_operation_keys_are_unique(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let old = case(&db, g, member_id(7), spam, None).await?;
    let new = case(&db, g, member_id(7), spam, None).await?;
    let mut conn = db.conn().await?;
    sqlx::query("UPDATE cases SET replaces_case_id = $2 WHERE id = $1")
        .bind(new.id.get())
        .bind(old.id.get())
        .execute(&mut *conn)
        .await?;
    let found = cases::replaced_by(&mut conn, g, old.id).await?;
    assert_eq!(found.map(|r| r.number), Some(new.number));
    let third = case(&db, g, member_id(7), spam, None).await?;
    let error = sqlx::query("UPDATE cases SET replaces_case_id = $2 WHERE id = $1")
        .bind(third.id.get())
        .bind(old.id.get())
        .execute(&mut *conn)
        .await
        .err()
        .map(quack_db::DbError::from);
    assert_eq!(
        error.and_then(|e| e.conflict()),
        Some(quack_db::Conflict::CaseAlreadyReplaced)
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn a_case_past_level_255_is_stored(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let decision = quack_core::case::CaseDecision {
        case_count: 256,
        level_index: 256,
        level_from_case_count: 256,
        ..common::decision(spam, None)
    };
    let id = quack_core::ids::CaseId::from_uuid(uuid::Uuid::now_v7());
    let mut tx = db.begin().await?;
    let number = quack_db::guilds::next_case_number(&mut tx, g).await?;
    quack_db::cases::insert(
        &mut tx,
        &quack_db::cases::NewCase {
            id,
            guild: g,
            number,
            member: common::member_id(7),
            member_joined_at: None,
            source: quack_core::case::CaseSource::Discord,
            moderator: Some(common::account_id(2)),
            decision: &decision,
            context: &quack_core::case::ContextValues::default(),
            operation_key: None,
            request_hash: None,
            replaces: None,
            created_at: time::OffsetDateTime::now_utc(),
        },
    )
    .await?;
    tx.commit().await?;
    let mut conn = db.conn().await?;
    let stored = quack_db::cases::get(&mut conn, g, id).await?;
    assert_eq!(
        stored.and_then(|c| c.decision).map(|d| d.level_index),
        Some(256)
    );
    Ok(())
}
