//! The status board (§21.6): one message in `#ops-status`, edited in
//! place, posted again when Discord no longer has it.

use quack_core::discord::RefusalKind;
use quack_db::DbError;
use quack_engine::port::{FileEdit, Nonce, Outcome};
use quack_text::Msg;
use quack_text::ops::{self as text, Backlog, Trend};
use time::OffsetDateTime;

use crate::Ops;

/// Reads everything the board shows.
pub(crate) async fn read(ops: &Ops) -> Result<text::Status, DbError> {
    let db = ops.engine().db();
    let mut conn = db.conn().await?;
    let servers = quack_db::ops::installed_count(&mut conn).await?;
    let backlog = quack_db::jobs::backlog(&mut conn).await?;
    let failed_actions = quack_db::actions::failed_by_kind(&mut conn)
        .await?
        .iter()
        .map(|(_, n)| n)
        .sum();
    let open_appeals = quack_db::appeals::count_open(&mut conn).await?;
    let open_tickets = quack_db::tickets::count_open(&mut conn, None).await?;
    let day = quack_db::fleet::day(&mut conn).await?;
    drop(conn);
    let runs = quack_telemetry::metrics().jobs.tally().read();
    let pool_size = db.pool().size();
    let idle = u32::try_from(db.pool().num_idle()).unwrap_or(u32::MAX);
    let gateway = ops.gateway().unwrap_or_default();
    Ok(text::Status {
        version: ops.config().version.clone(),
        commit: ops.config().commit.clone(),
        started_at: ops.inner.started_at,
        gateway_connected: gateway.connected,
        latency_ms: gateway
            .latency
            .map(|l| u64::try_from(l.as_millis()).unwrap_or(u64::MAX)),
        servers,
        members: day.members,
        trend: Trend {
            cases: day.cases,
            appeals: day.appeals,
            tickets: day.tickets,
            joins: day.joins,
            leaves: day.leaves,
            job_runs: runs.kinds.iter().map(|k| k.runs).sum(),
            job_failures: runs.kinds.iter().map(|k| k.retries + k.dead).sum(),
            runs_seconds: runs.covers.as_secs(),
        },
        dead_jobs: backlog.iter().map(|b| b.dead).sum(),
        backlog: backlog
            .iter()
            .filter(|b| b.ready > 0)
            .map(|b| Backlog {
                kind: b.kind.as_str(),
                ready: b.ready,
                oldest_seconds: b.oldest_ready_age.as_secs(),
            })
            .collect(),
        failed_actions,
        open_appeals,
        open_tickets,
        pool_used: pool_size.saturating_sub(idle),
        pool_size,
        updated_at: OffsetDateTime::now_utc(),
    })
}

/// The board as a message, with **Refresh**.
pub(crate) async fn message(ops: &Ops) -> Result<Msg, DbError> {
    Ok(text::status(&read(ops).await?, true))
}

/// Edits the board, or posts it when there is none (or Discord lost it).
pub(crate) async fn update(ops: &Ops) -> Result<(), DbError> {
    let Some(channel) = ops.channels().and_then(|c| c.status) else {
        return Ok(());
    };
    let msg = message(ops).await?;
    let discord = ops.engine().discord();
    let stored = {
        let mut conn = ops.engine().db().conn().await?;
        quack_db::ops::status_message(&mut conn).await?
    };
    if let Some((stored_channel, message)) = stored
        && stored_channel == channel
    {
        match discord.edit(channel, message, &msg, &FileEdit::None).await {
            Outcome::Done(_) => return Ok(()),
            Outcome::Refused(r)
                if matches!(
                    r.kind,
                    RefusalKind::UnknownMessage | RefusalKind::UnknownChannel
                ) => {}
            other => {
                tracing::warn!(outcome = %other.code(), "couldn't edit the ops status board");
                return Ok(());
            }
        }
    }
    let nonce = Nonce::new(format!("st{}", OffsetDateTime::now_utc().unix_timestamp()));
    match discord.send(channel, &msg, &[], &nonce).await {
        Outcome::Done(sent) => {
            let mut conn = ops.engine().db().conn().await?;
            quack_db::ops::set_status_message(&mut conn, channel, sent.message_id).await?;
        }
        other => tracing::warn!(outcome = %other.code(), "couldn't post the ops status board"),
    }
    Ok(())
}
