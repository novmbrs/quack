//! Closing a ticket (§13.2.4): the member or a moderator (button `tc` or
//! the dashboard) moves an `open` ticket to `closing` with its job.

use quack_core::DomainError;
use quack_core::ids::TicketId;
use quack_core::modules::TicketState;
use quack_core::permissions::Capability;
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::tickets::{self, Ticket};
use quack_engine::opkey::{KeyError, Keyed, OpKey, begin_keyed, complete, request_hash};
use quack_engine::{Engine, Principal};
use quack_text::Text;

use crate::TicketError;
use crate::obs::span;

/// What the closer is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAnswer {
    /// Quack is closing it and saving the transcript.
    Closing,
}

impl CloseAnswer {
    /// The ephemeral reply (`ticket.close.closing`).
    pub fn text(self) -> Text {
        quack_text::ticket::close_closing()
    }
}

/// Closes an `open` ticket for its member, or for a moderator (`moderate`
/// and 2FA). Anyone else is [`TicketError::NotAllowed`]; a ticket that is
/// not open is `ticket_not_open`. `op_key` (the interaction or the
/// `Idempotency-Key`) makes a repeat of the same request answer like the
/// first, after the caller is authorized again.
pub async fn close(
    engine: &Engine,
    principal: &Principal,
    ticket: TicketId,
    op_key: &OpKey,
) -> Result<CloseAnswer, TicketError> {
    let guild = principal.guild;
    span("close_ticket", guild, async {
        let mut conn = engine.db().conn().await?;
        let t = tickets::get(&mut conn, guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        authorize(&mut conn, principal, &t).await?;
        drop(conn);
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let mut tx = match begin_keyed(engine.db(), &key, &request_hash(&ticket)).await {
            Ok(Keyed::Replay(_)) => return Ok(CloseAnswer::Closing),
            Ok(Keyed::Fresh(tx)) => tx,
            Err(KeyError::Reused) => return Err(TicketError::OperationKeyReused),
            Err(KeyError::Db(e)) => return Err(e.into()),
        };
        let locked = tickets::lock(&mut tx, guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        if locked.state != TicketState::Open
            || !tickets::request_close(&mut tx, guild, ticket, Some(principal.account)).await?
        {
            return Err(DomainError::TicketNotOpen.into());
        }
        jobs::enqueue(&mut tx, &NewJob::close_ticket(guild, ticket)).await?;
        messages::bump(&mut tx, guild, Owner::Ticket(ticket)).await?;
        complete(&mut tx, &key, 200, serde_json::json!({ "ticket": ticket })).await?;
        tx.commit().await?;
        tracing::info!(ticket_id = %ticket.get(), "ticket closing");
        Ok(CloseAnswer::Closing)
    })
    .await
}

/// The ticket's member may always act on it; anyone else needs `moderate`
/// (and 2FA when the server requires it).
pub(crate) async fn authorize(
    conn: &mut quack_db::PgConnection,
    principal: &Principal,
    t: &Ticket,
) -> Result<(), TicketError> {
    if principal.account == t.member.account() {
        return Ok(());
    }
    match principal.require(conn, Capability::Moderate).await? {
        Ok(_) => Ok(()),
        Err(DomainError::MissingCapability(_)) => Err(TicketError::NotAllowed),
        Err(other) => Err(other.into()),
    }
}
