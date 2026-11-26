//! The status boards (§21.6): the ops board in `#ops-status` and, when
//! `QUACK_OPS_STATUS_CHANNEL_ID` is set, the public board. Each is one
//! message with a card image, edited in place and posted again when
//! Discord no longer has it.

use quack_core::discord::RefusalKind;
use quack_core::ids::ChannelId;
use quack_db::DbError;
use quack_db::ops::Board;
use quack_engine::port::{FileEdit, Nonce, Outcome};
use quack_text::ops::{self as text, Backlog, Trend};
use time::OffsetDateTime;

use crate::Ops;
use crate::card::{Rendered, attach};

/// Reads everything the boards show.
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
    let cases_hourly = quack_db::fleet::hourly_cases(&mut conn).await?;
    drop(conn);
    let tally = quack_telemetry::metrics().jobs.tally();
    let runs = tally.read();
    // Capacity, not the connections open now: a pool that opened 2 of
    // its 20 isn't full when both are busy.
    let pool_size = db.pool().options().get_max_connections();
    let open = db.pool().size();
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
            cases_hourly,
            runs_hourly: tally.hourly(),
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
        pool_used: open.saturating_sub(idle),
        pool_size,
        updated_at: OffsetDateTime::now_utc(),
    })
}

/// The ops board, with **Refresh**.
pub(crate) async fn message(ops: &Ops) -> Result<Rendered, DbError> {
    let status = read(ops).await?;
    Ok(ops_board(&status).await)
}

async fn ops_board(status: &text::Status) -> Rendered {
    attach(
        text::status(status, true),
        text::status_card(status),
        text::STATUS_IMAGE,
    )
    .await
}

async fn public_board(status: &text::Status) -> Rendered {
    attach(
        text::public_status(status),
        text::public_card(status),
        text::PUBLIC_IMAGE,
    )
    .await
}

/// Edits both boards, or posts one that is missing (or that Discord
/// lost). One read serves both.
pub(crate) async fn update(ops: &Ops) -> Result<(), DbError> {
    let ops_channel = ops.channels().and_then(|c| c.status);
    let public_channel = ops.config().public_status;
    if ops_channel.is_none() && public_channel.is_none() {
        return Ok(());
    }
    let status = read(ops).await?;
    if let Some(channel) = ops_channel {
        put(ops, Board::Ops, channel, ops_board(&status).await).await?;
    }
    if let Some(channel) = public_channel {
        put(ops, Board::Public, channel, public_board(&status).await).await?;
    }
    Ok(())
}

/// Edits `board`'s message in `channel`, or posts it.
async fn put(ops: &Ops, board: Board, channel: ChannelId, r: Rendered) -> Result<(), DbError> {
    let discord = ops.engine().discord();
    let stored = {
        let mut conn = ops.engine().db().conn().await?;
        quack_db::ops::status_message(&mut conn, board).await?
    };
    if let Some((stored_channel, message)) = stored
        && stored_channel == channel
    {
        let files = r
            .files
            .first()
            .cloned()
            .map_or(FileEdit::None, FileEdit::Replace);
        match discord.edit(channel, message, &r.msg, &files).await {
            Outcome::Done(_) => return Ok(()),
            Outcome::Refused(refusal)
                if matches!(
                    refusal.kind,
                    RefusalKind::UnknownMessage | RefusalKind::UnknownChannel
                ) => {}
            other => {
                tracing::warn!(outcome = %other.code(), ?board, "couldn't edit a status board");
                return Ok(());
            }
        }
    }
    let prefix = match board {
        Board::Ops => "st",
        Board::Public => "ps",
    };
    let nonce = Nonce::new(format!(
        "{prefix}{}",
        OffsetDateTime::now_utc().unix_timestamp()
    ));
    match discord.send(channel, &r.msg, &r.files, &nonce).await {
        Outcome::Done(sent) => {
            let mut conn = ops.engine().db().conn().await?;
            quack_db::ops::set_status_message(&mut conn, board, channel, sent.message_id).await?;
        }
        other => tracing::warn!(outcome = %other.code(), ?board, "couldn't post a status board"),
    }
    Ok(())
}
