//! Shared fixtures for the quack-db integration tests.
//!
//! Every test runs in its own database created by `#[sqlx::test]` from
//! `DATABASE_URL` (CI's PostgreSQL 18 service, or a local container) with
//! the crate's migrations applied.

#![allow(dead_code, unreachable_pub, missing_docs)]

use std::sync::LazyLock;

use anyhow::{Context as _, Result};
use quack_core::case::{CaseDecision, CaseSource, ContextValues};
use quack_core::ids::{AccountId, CaseId, GuildId, MemberId, RuleId};
use quack_core::rule::{LevelAction, RuleDefinition};
use quack_db::cases::{self, Case, NewCase};
use quack_db::guilds::{self, GuildFacts};
use quack_db::rules::{self, NewRule};
use quack_db::{Db, Tx};
use sqlx::postgres::PgConnectOptions;
use time::OffsetDateTime;
use uuid::Uuid;

/// Tests that open dozens of connections take this so they don't run at
/// once and exhaust the server's `max_connections`.
pub static HEAVY: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// A small pool on the test's database.
pub async fn db(options: PgConnectOptions) -> Result<Db> {
    Ok(Db::connect_with(options, 4).await?)
}

/// A pool with room for `connections` concurrent transactions.
pub async fn db_with(options: PgConnectOptions, connections: u32) -> Result<Db> {
    Ok(Db::connect_with(options, connections).await?)
}

pub fn guild_id(n: u64) -> GuildId {
    GuildId::new(n).unwrap_or_else(|| unreachable!("test ids are positive"))
}

pub fn member_id(n: u64) -> MemberId {
    MemberId::new(n).unwrap_or_else(|| unreachable!("test ids are positive"))
}

pub fn account_id(n: u64) -> AccountId {
    AccountId::new(n).unwrap_or_else(|| unreachable!("test ids are positive"))
}

/// Installs a server with all its child rows.
pub async fn guild(db: &Db, n: u64) -> Result<GuildId> {
    let id = guild_id(n);
    let mut tx = db.begin().await?;
    guilds::upsert(
        &mut tx,
        &GuildFacts {
            id,
            name: format!("Server {n}"),
            icon_hash: None,
            owner_id: account_id(1),
            mfa_required: false,
        },
    )
    .await?;
    guilds::ensure_children(&mut tx, id).await?;
    tx.commit().await?;
    Ok(id)
}

/// A rule definition: one level per action, starting at case counts 1, 2, 3.
pub fn definition(name: &str, levels: &[Option<LevelAction>]) -> Result<RuleDefinition> {
    let levels: Vec<serde_json::Value> = levels
        .iter()
        .enumerate()
        .map(|(i, action)| serde_json::json!({ "from_case_count": i + 1, "action": action, "send_dm": true }))
        .collect();
    serde_json::from_value(serde_json::json!({
        "name": name,
        "description": "",
        "reason": "Please follow the server rules.",
        "appealable": true,
        "counting_window_days": null,
        "fields": [],
        "levels": levels,
    }))
    .context("rule definition")
}

/// Creates a warning-only rule.
pub async fn rule(db: &Db, guild: GuildId, name: &str) -> Result<(RuleId, RuleDefinition)> {
    let definition = definition(name, &[None])?;
    let id = RuleId::from_uuid(Uuid::now_v7());
    let mut tx = db.begin().await?;
    rules::create(
        &mut tx,
        &NewRule {
            id,
            guild,
            definition: definition.clone(),
            created_by: None,
            reconstructed: false,
        },
    )
    .await?;
    tx.commit().await?;
    Ok((id, definition))
}

/// The decision for a case under version 1 of `rule`.
pub fn decision(rule: RuleId, action: Option<LevelAction>) -> CaseDecision {
    CaseDecision {
        rule_id: rule,
        rule_version: 1,
        case_count: 1,
        level_index: 1,
        level_from_case_count: 1,
        action,
        sends_dm: true,
    }
}

/// Inserts a discord case in `tx` (number from the counter) and returns it.
pub async fn insert_case(
    tx: &mut Tx,
    guild: GuildId,
    member: MemberId,
    rule: RuleId,
    action: Option<LevelAction>,
) -> Result<CaseId> {
    let number = guilds::next_case_number(tx, guild).await?;
    let id = CaseId::from_uuid(Uuid::now_v7());
    let decision = decision(rule, action);
    let context = ContextValues::default();
    cases::insert(
        tx,
        &NewCase {
            id,
            guild,
            number,
            member,
            member_joined_at: None,
            source: CaseSource::Discord,
            moderator: Some(account_id(2)),
            decision: &decision,
            context: &context,
            operation_key: None,
            request_hash: None,
            replaces: None,
            created_at: OffsetDateTime::now_utc(),
        },
    )
    .await?;
    Ok(id)
}

/// Creates and commits a case, returning the stored row.
pub async fn case(
    db: &Db,
    guild: GuildId,
    member: MemberId,
    rule: RuleId,
    action: Option<LevelAction>,
) -> Result<Case> {
    let mut tx = db.begin().await?;
    let id = insert_case(&mut tx, guild, member, rule, action).await?;
    tx.commit().await?;
    let mut conn = db.conn().await?;
    cases::get(&mut conn, guild, id)
        .await?
        .context("case just created")
}

/// The SQLSTATE of a failed raw statement.
pub fn sqlstate(error: &sqlx::Error) -> Option<String> {
    match error {
        sqlx::Error::Database(db) => db.code().map(std::borrow::Cow::into_owned),
        _ => None,
    }
}
