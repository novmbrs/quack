//! Closing a ticket (§13.2.4): the member or a moderator (button `tc` or
//! the dashboard) moves an `open` ticket to `closing` with its job.

use quack_core::DomainError;
use quack_core::ids::TicketId;
use quack_core::modules::TicketState;
use quack_core::permissions::Capability;
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_db::tickets::{self, Ticket};
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
/// not open is `ticket_not_open`.
pub async fn close(
    engine: &Engine,
    principal: &Principal,
    ticket: TicketId,
) -> Result<CloseAnswer, TicketError> {
    let guild = principal.guild;
    span("close_ticket", guild, async {
        let mut conn = engine.db().conn().await?;
        let t = tickets::get(&mut conn, guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        authorize(&mut conn, principal, &t).await?;
        drop(conn);
        let mut tx = engine.db().begin().await?;
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
