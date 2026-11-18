//! A member selects **Open a ticket** (§13.2.3): one insert and the
//! `open_ticket` job; the job does the rest.

use quack_core::ids::{ChannelId, GuildId, MemberId, TicketId};
use quack_core::modules::TicketState;
use quack_db::jobs::{self, NewJob};
use quack_db::{Conflict, tickets};
use quack_engine::Engine;
use quack_text::Text;
use quack_text::model::ChannelRef;

use crate::TicketError;
use crate::obs::span;

/// What the member is told after selecting **Open a ticket**.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAnswer {
    /// Quack is opening it (also the answer while it is still opening).
    Opening {
        /// The entry channel the thread is created in.
        entry: ChannelId,
        /// The ticket.
        ticket: TicketId,
    },
    /// The member already has an open ticket in this thread.
    Exists {
        /// Its thread.
        thread: ChannelId,
    },
    /// The member's previous ticket is still closing.
    Closing,
    /// Tickets are turned off.
    Disabled,
}

impl OpenAnswer {
    /// The ephemeral reply (`ticket.open.*`).
    pub fn text(&self) -> Text {
        match self {
            Self::Opening { entry, .. } => quack_text::ticket::open_opening(&channel(*entry)),
            Self::Exists { thread } => quack_text::ticket::open_exists(&channel(*thread)),
            Self::Closing => quack_text::ticket::open_closing(),
            Self::Disabled => quack_text::ticket::open_disabled(),
        }
    }
}

/// A channel shown as a mention.
pub(crate) fn channel(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}

/// Opens a ticket for `member` (component `to`): inserts it `opening` with
/// its `open_ticket` job in one transaction. The partial unique index on
/// live tickets makes a double click, or two clicks racing, answer with the
/// ticket the first one made.
pub async fn open(
    engine: &Engine,
    guild: GuildId,
    member: MemberId,
) -> Result<OpenAnswer, TicketError> {
    span("open_ticket", guild, async {
        let mut conn = engine.db().conn().await?;
        let entry = tickets::settings(&mut conn, guild)
            .await?
            .filter(|s| s.enabled)
            .and_then(|s| s.entry_channel);
        let Some(entry) = entry else {
            return Ok(OpenAnswer::Disabled);
        };
        drop(conn);
        let ticket = TicketId::from_uuid(uuid::Uuid::now_v7());
        let mut tx = engine.db().begin().await?;
        match tickets::insert(&mut tx, ticket, guild, member, entry).await {
            Ok(()) => {}
            Err(e) if e.conflict() == Some(Conflict::LiveTicketExists) => {
                tx.rollback().await?;
                return live_answer(engine, guild, member).await;
            }
            Err(e) => return Err(e.into()),
        }
        jobs::enqueue(&mut tx, &NewJob::open_ticket(guild, ticket)).await?;
        tx.commit().await?;
        tracing::info!(ticket_id = %ticket.get(), member_id = member.get(), "ticket opening");
        Ok(OpenAnswer::Opening { entry, ticket })
    })
    .await
}

/// The answer for a member who already has a live ticket.
async fn live_answer(
    engine: &Engine,
    guild: GuildId,
    member: MemberId,
) -> Result<OpenAnswer, TicketError> {
    let mut conn = engine.db().conn().await?;
    let Some(live) = tickets::live_for_member(&mut conn, guild, member).await? else {
        // It closed in between: the member may simply select again.
        return Ok(OpenAnswer::Closing);
    };
    Ok(match (live.state, live.thread) {
        (TicketState::Open, Some(thread)) => OpenAnswer::Exists { thread },
        (TicketState::Closing | TicketState::Closed, _) => OpenAnswer::Closing,
        // A ticket still opening always has its entry channel.
        _ => match live.entry_channel {
            Some(entry) => OpenAnswer::Opening {
                entry,
                ticket: live.id,
            },
            None => OpenAnswer::Closing,
        },
    })
}
