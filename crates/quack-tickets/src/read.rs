//! Reading tickets and transcripts (the dashboard, §13.2.5).

use quack_core::ids::{AccountId, ChannelId, MemberId, TicketId};
use quack_core::modules::{TicketCloseReason, TicketState, TranscriptPartial};
use quack_core::permissions::Capability;
use quack_db::TimeCursor;
use quack_db::tickets::{self, Ticket};
use quack_engine::{Engine, Principal};
use serde::Serialize;
use time::OffsetDateTime;

use crate::TicketError;
use crate::close::authorize;
use crate::obs::span;

/// A ticket as the API shows it (§14.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TicketView {
    /// The ticket.
    pub id: TicketId,
    /// Its member.
    pub member_id: MemberId,
    /// Its state.
    pub state: TicketState,
    /// Its private thread, once created.
    pub thread_id: Option<ChannelId>,
    /// The channel the thread is in.
    pub entry_channel_id: ChannelId,
    /// When it was requested.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// When it became open.
    #[serde(with = "time::serde::rfc3339::option")]
    pub opened_at: Option<OffsetDateTime>,
    /// When closing was requested.
    #[serde(with = "time::serde::rfc3339::option")]
    pub close_requested_at: Option<OffsetDateTime>,
    /// When it closed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub closed_at: Option<OffsetDateTime>,
    /// Who closed it; `None` when Quack did (or it is not closed).
    pub closed_by: Option<AccountId>,
    /// `open_failed` when it closed without opening.
    pub close_reason: Option<TicketCloseReason>,
    /// Quack couldn't delete its thread.
    pub thread_delete_failed: bool,
}

impl From<Ticket> for TicketView {
    fn from(t: Ticket) -> Self {
        Self {
            id: t.id,
            member_id: t.member,
            state: t.state,
            thread_id: t.thread,
            entry_channel_id: t.entry_channel,
            created_at: t.created_at,
            opened_at: t.opened_at,
            close_requested_at: t.close_requested_at,
            closed_at: t.closed_at,
            closed_by: t.closed_by,
            close_reason: t.close_reason,
            thread_delete_failed: t.thread_delete_failed,
        }
    }
}

/// A saved transcript (§13.2.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TranscriptView {
    /// The plain-text transcript.
    pub content: String,
    /// Why it may be missing messages; `None` when complete.
    pub partial: Option<TranscriptPartial>,
    /// When it was saved.
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
    /// When it is deleted.
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// A server's tickets, newest first (`moderate` + 2FA).
pub async fn list(
    engine: &Engine,
    principal: &Principal,
    state: Option<TicketState>,
    cursor: Option<TimeCursor>,
    page: u32,
) -> Result<Vec<TicketView>, TicketError> {
    span("list_tickets", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        principal.require(&mut conn, Capability::Moderate).await??;
        let rows = tickets::list(&mut conn, principal.guild, state, cursor, page).await?;
        Ok(rows.into_iter().map(TicketView::from).collect())
    })
    .await
}

/// One ticket, for its member or `moderate` + 2FA.
pub async fn get(
    engine: &Engine,
    principal: &Principal,
    ticket: TicketId,
) -> Result<TicketView, TicketError> {
    span("get_ticket", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        let t = tickets::get(&mut conn, principal.guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        authorize(&mut conn, principal, &t).await?;
        Ok(TicketView::from(t))
    })
    .await
}

/// A ticket's transcript until it expires, for its member or `moderate` +
/// 2FA; after that [`TicketError::TranscriptUnavailable`] (`file_unavailable`).
pub async fn transcript(
    engine: &Engine,
    principal: &Principal,
    ticket: TicketId,
) -> Result<TranscriptView, TicketError> {
    span("get_ticket_transcript", principal.guild, async {
        let mut conn = engine.db().conn().await?;
        let t = tickets::get(&mut conn, principal.guild, ticket)
            .await?
            .ok_or(TicketError::NotFound)?;
        authorize(&mut conn, principal, &t).await?;
        let saved = tickets::transcript(&mut conn, principal.guild, ticket)
            .await?
            .ok_or(TicketError::TranscriptUnavailable)?;
        Ok(TranscriptView {
            content: saved.content,
            partial: saved.partial,
            captured_at: saved.captured_at,
            expires_at: saved.expires_at,
        })
    })
    .await
}
