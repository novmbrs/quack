//! Shared helpers for the engine scenario tests.

#![allow(dead_code, unreachable_pub, missing_docs)]

use anyhow::{Context as _, Result};
use quack_core::ids::{ActionId, CaseId, DmId};
use quack_core::modlog::ModLogKind;
use quack_db::actions::Action;
use quack_db::dms::Dm;
use quack_db::modlog::{self, ModLogFilter};
use quack_engine::CaseRef;
use quack_engine::cases::void_case;
use quack_testkit::Harness;

pub async fn action(h: &Harness, id: ActionId) -> Result<Action> {
    let mut conn = h.db.db.conn().await?;
    quack_db::actions::get(&mut conn, h.guild, id)
        .await?
        .context("action exists")
}

pub async fn undo_of(h: &Harness, original: ActionId) -> Result<Option<Action>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::actions::undo_of(&mut conn, h.guild, original).await?)
}

pub async fn dm(h: &Harness, id: DmId) -> Result<Dm> {
    let mut conn = h.db.db.conn().await?;
    quack_db::dms::get(&mut conn, h.guild, id)
        .await?
        .context("dm exists")
}

pub async fn void(h: &Harness, case: CaseId) -> Result<()> {
    void_case(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        "A mistake.",
        &h.key("vdm"),
    )
    .await?;
    Ok(())
}

/// The kinds of the server's moderation log entries, oldest first.
pub async fn log_kinds(h: &Harness) -> Result<Vec<ModLogKind>> {
    let mut conn = h.db.db.conn().await?;
    let entries = modlog::list(&mut conn, h.guild, &ModLogFilter::default(), None, 100).await?;
    Ok(entries.into_iter().rev().map(|e| e.entry.kind()).collect())
}

/// How many entries of `kind` the log has.
pub async fn logged(h: &Harness, kind: ModLogKind) -> Result<usize> {
    Ok(log_kinds(h)
        .await?
        .into_iter()
        .filter(|k| *k == kind)
        .count())
}
