//! Tracked messages (§10): Discord messages Quack keeps up to date, one row
//! each, synced by revision.

use time::{Duration, OffsetDateTime};

use crate::InvalidTransition;
use crate::ids::{ChannelId, MessageRowId};
use crate::modules::ModuleName;

str_enum! {
    /// What a tracked message shows (`message_kind`).
    pub enum MessageKind {
        /// A case's receipt.
        CaseReceipt = "case_receipt",
        /// The staff post for an appeal.
        AppealPost = "appeal_post",
        /// The server's "Open a ticket" panel.
        TicketPanel = "ticket_panel",
        /// The staff post for a ticket.
        TicketPost = "ticket_post",
        /// The honeypot channel's warning.
        HoneypotWarning = "honeypot_warning",
    }
}

impl MessageKind {
    /// Whether rows of this kind belong to a case, appeal, or ticket
    /// (`owner_id` set) rather than being one per server.
    pub const fn has_owner(self) -> bool {
        !matches!(self, Self::TicketPanel | Self::HoneypotWarning)
    }
}

str_enum! {
    /// A tracked message's state (`message_state`).
    pub enum MessageState {
        /// Kept up to date.
        Active = "active",
        /// No longer synced; bumps skip it.
        Gone = "gone",
    }
}

impl MessageState {
    /// Marks the message gone; a gone message never comes back (a re-post
    /// is a new `message_id` on an active row instead).
    pub fn mark_gone(self) -> Result<Self, InvalidTransition> {
        match self {
            Self::Active => Ok(Self::Gone),
            Self::Gone => Err(InvalidTransition {
                machine: "message",
                from: "gone",
                event: "mark_gone",
            }),
        }
    }
}

/// How long an interaction token edits a response: 15 minutes minus a
/// 60-second safety margin.
pub const INTERACTION_TOKEN_LIFETIME: Duration = Duration::seconds(15 * 60 - 60);
/// Most render-and-write loops per sync job run.
pub const MAX_SYNC_LOOPS: u32 = 3;
/// Attempts per `sync_message` job.
pub const SYNC_ATTEMPTS: u32 = 6;
/// Waits between `sync_message` attempts.
pub const SYNC_BACKOFF: [Duration; 5] = [
    Duration::seconds(2),
    Duration::seconds(10),
    Duration::minutes(1),
    Duration::minutes(5),
    Duration::minutes(30),
];

/// When an interaction token received at `received_at` stops being used.
pub fn token_expires_at(received_at: OffsetDateTime) -> OffsetDateTime {
    received_at + INTERACTION_TOKEN_LIFETIME
}

/// Whether a sync is needed: Discord shows an older revision than wanted.
pub const fn needs_sync(wanted_revision: u32, posted_revision: u32) -> bool {
    wanted_revision > posted_revision
}

/// How the sync job writes a message (§10.3 step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncRoute {
    /// Edit the original interaction response through the token; store the
    /// returned message id.
    EditOriginal,
    /// Create the message with the revision's nonce.
    Create,
    /// Edit the existing message through the token.
    EditWithToken,
    /// Edit the existing message with bot credentials.
    EditWithBot,
    /// An ephemeral receipt whose token expired can't be edited: mark gone.
    MarkGone,
}

/// Picks the write for a sync.
pub const fn sync_route(posted: bool, token_valid: bool, private: bool) -> SyncRoute {
    match (posted, token_valid) {
        _ if private && !token_valid => SyncRoute::MarkGone,
        (false, true) => SyncRoute::EditOriginal,
        (false, false) => SyncRoute::Create,
        (true, true) => SyncRoute::EditWithToken,
        (true, false) => SyncRoute::EditWithBot,
    }
}

/// The nonce for creating revision `rev` of a tracked message: its row's
/// u22 plus `rev mod 1000` as 3 digits (25 characters).
pub fn create_nonce(row: MessageRowId, rev: u32) -> String {
    format!("{}{:03}", row.to_u22(), rev % 1000)
}

/// Discord error codes meaning the message or its channel is gone:
/// Unknown Message, Unknown Channel, Missing Access.
pub const fn is_missing(code: Option<u32>) -> bool {
    matches!(code, Some(10_008 | 10_003 | 50_001))
}

/// The machine code stored in `last_error` for a failed sync, e.g.
/// `http_403_50001`. Never shown to people.
pub fn sync_error_code(status: u16, code: Option<u32>) -> String {
    match code {
        Some(code) => format!("http_{status}_{code}"),
        None => format!("http_{status}"),
    }
}

/// What to do when a tracked message or its channel is gone (§10.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingPlan {
    /// Post it again in this channel (a new `message_id`).
    Repost(ChannelId),
    /// Mark the row gone, and turn this module off when it needs the
    /// message.
    MarkGone {
        /// The module to disable (`channel_deleted`), if any.
        disable: Option<ModuleName>,
    },
}

/// Decides what happens to a missing message of `kind`, given the channel
/// it belongs in now (the current appeal, entry, queue, or honeypot
/// channel setting; `None` when unset or also gone).
pub const fn on_missing(kind: MessageKind, current_channel: Option<ChannelId>) -> MissingPlan {
    match (kind, current_channel) {
        (MessageKind::CaseReceipt, _) => MissingPlan::MarkGone { disable: None },
        (_, Some(channel)) => MissingPlan::Repost(channel),
        (MessageKind::TicketPanel, None) => MissingPlan::MarkGone {
            disable: Some(ModuleName::Tickets),
        },
        (MessageKind::HoneypotWarning, None) => MissingPlan::MarkGone {
            disable: Some(ModuleName::Honeypot),
        },
        (MessageKind::AppealPost | MessageKind::TicketPost, None) => {
            MissingPlan::MarkGone { disable: None }
        }
    }
}

/// What happens to the old message after a settings change moved a
/// tracked message to another channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleCleanup {
    /// Edit it to say the panel moved (`ticket.panel.moved`).
    EditToMoved,
    /// Delete it.
    Delete,
}

/// The cleanup for a moved message of `kind`; only the ticket panel and
/// the honeypot warning move.
pub const fn stale_cleanup(kind: MessageKind) -> Option<StaleCleanup> {
    match kind {
        MessageKind::TicketPanel => Some(StaleCleanup::EditToMoved),
        MessageKind::HoneypotWarning => Some(StaleCleanup::Delete),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn owners() {
        assert!(MessageKind::CaseReceipt.has_owner());
        assert!(MessageKind::TicketPost.has_owner());
        assert!(!MessageKind::TicketPanel.has_owner());
        assert!(!MessageKind::HoneypotWarning.has_owner());
    }

    #[test]
    fn gone_is_final() {
        assert_eq!(MessageState::Active.mark_gone(), Ok(MessageState::Gone));
        assert!(MessageState::Gone.mark_gone().is_err());
    }

    #[test]
    fn routes() {
        let cases = [
            (false, true, false, SyncRoute::EditOriginal),
            (false, false, false, SyncRoute::Create),
            (true, true, false, SyncRoute::EditWithToken),
            (true, false, false, SyncRoute::EditWithBot),
            (false, true, true, SyncRoute::EditOriginal),
            (true, true, true, SyncRoute::EditWithToken),
            (true, false, true, SyncRoute::MarkGone),
            (false, false, true, SyncRoute::MarkGone),
        ];
        for (posted, token, private, route) in cases {
            assert_eq!(
                sync_route(posted, token, private),
                route,
                "{posted} {token} {private}"
            );
        }
    }

    #[test]
    fn revisions_tokens_and_nonces() {
        assert!(needs_sync(2, 1));
        assert!(!needs_sync(1, 1));
        let at = datetime!(2026-11-17 12:00 UTC);
        assert_eq!(token_expires_at(at), datetime!(2026-11-17 12:14 UTC));
        let row = MessageRowId::from_uuid(Uuid::from_u128(0));
        assert_eq!(create_nonce(row, 1), "AAAAAAAAAAAAAAAAAAAAAA001");
        assert_eq!(create_nonce(row, 2_345).len(), 25);
        assert!(create_nonce(row, 2_345).ends_with("345"));
    }

    #[test]
    fn missing_handling() {
        assert!(is_missing(Some(10_008)) && is_missing(Some(10_003)) && is_missing(Some(50_001)));
        assert!(!is_missing(Some(50_013)) && !is_missing(None));
        assert_eq!(sync_error_code(403, Some(50_001)), "http_403_50001");
        assert_eq!(sync_error_code(400, None), "http_400");
        let ch = ChannelId::new(9).unwrap();
        assert_eq!(
            on_missing(MessageKind::CaseReceipt, Some(ch)),
            MissingPlan::MarkGone { disable: None }
        );
        assert_eq!(
            on_missing(MessageKind::AppealPost, Some(ch)),
            MissingPlan::Repost(ch)
        );
        assert_eq!(
            on_missing(MessageKind::AppealPost, None),
            MissingPlan::MarkGone { disable: None }
        );
        assert_eq!(
            on_missing(MessageKind::TicketPanel, None),
            MissingPlan::MarkGone {
                disable: Some(ModuleName::Tickets)
            }
        );
        assert_eq!(
            on_missing(MessageKind::TicketPost, None),
            MissingPlan::MarkGone { disable: None }
        );
        assert_eq!(
            on_missing(MessageKind::HoneypotWarning, None),
            MissingPlan::MarkGone {
                disable: Some(ModuleName::Honeypot)
            }
        );
        assert_eq!(
            stale_cleanup(MessageKind::TicketPanel),
            Some(StaleCleanup::EditToMoved)
        );
        assert_eq!(
            stale_cleanup(MessageKind::HoneypotWarning),
            Some(StaleCleanup::Delete)
        );
        assert_eq!(stale_cleanup(MessageKind::CaseReceipt), None);
    }
}
