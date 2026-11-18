//! Tracked messages (§10): Discord messages Quack keeps up to date, one row
//! each, synced by revision.

use time::{Duration, OffsetDateTime};

use crate::InvalidTransition;
use crate::ids::{ChannelId, MessageRowId};

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
    /// Whether rows of this kind belong to a case, appeal, or ticket (the
    /// matching owner column is set) rather than being one per server.
    pub const fn has_owner(self) -> bool {
        !matches!(self, Self::TicketPanel | Self::HoneypotWarning)
    }
}

/// Most earlier locations a row keeps for clean-up after moves (`stale`).
pub const MAX_STALE: usize = 5;

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

/// When an interaction token received at `received_at` stops being used;
/// `None` (the token can't be used) when that can't be represented.
pub fn token_expires_at(received_at: OffsetDateTime) -> Option<OffsetDateTime> {
    received_at.checked_add(INTERACTION_TOKEN_LIFETIME)
}

/// Whether a sync is needed: Discord shows an older revision than wanted.
pub const fn needs_sync(wanted_revision: u32, posted_revision: u32) -> bool {
    wanted_revision > posted_revision
}

/// How the sync job writes a message (§10.3 step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncRoute {
    /// A private receipt: edit the interaction's original response
    /// through its token.
    EditOriginal,
    /// A private receipt whose token expired: mark it `gone`; nothing else
    /// is ever done with it.
    MarkGone,
    /// No channel yet (an appeal post waiting for an appeal channel):
    /// nothing to write.
    Wait,
    /// Edit the posted message with bot credentials, keeping its
    /// attachment.
    EditWithBot,
    /// Create the message (§10.4) with [`create_nonce`].
    Create,
}

/// What the sync's read step knows about a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about one row"
)]
pub struct SyncRow {
    /// A private receipt (token edits only).
    pub private: bool,
    /// Whether the interaction token is still valid.
    pub token_valid: bool,
    /// Whether `channel_id` is set.
    pub has_channel: bool,
    /// Whether `message_id` is set.
    pub posted: bool,
}

/// Picks the write for a sync.
pub const fn sync_route(row: SyncRow) -> SyncRoute {
    match row {
        SyncRow {
            private: true,
            token_valid: true,
            ..
        } => SyncRoute::EditOriginal,
        SyncRow { private: true, .. } => SyncRoute::MarkGone,
        SyncRow {
            has_channel: false, ..
        } => SyncRoute::Wait,
        SyncRow { posted: true, .. } => SyncRoute::EditWithBot,
        SyncRow { .. } => SyncRoute::Create,
    }
}

/// The nonce for creating a tracked message: its row's u22 plus the
/// generation in base 36 (at most 3 digits, so at most 25 characters).
/// Stable across revisions, new for each generation (a move, a repost, or
/// a reactivation).
pub fn create_nonce(row: MessageRowId, generation: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = generation % (36 * 36 * 36);
    let mut digits = Vec::with_capacity(3);
    loop {
        digits.push(char::from(DIGITS[(n % 36) as usize]));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    let mut nonce = row.to_u22();
    nonce.extend(digits.iter().rev());
    nonce
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
    /// Post it again in this channel (`generation + 1`).
    Repost(ChannelId),
    /// An appeal post with no appeal channel set: wait with `channel_id`
    /// NULL.
    WaitForChannel,
    /// A public receipt whose create was refused while the token is valid:
    /// it becomes a private receipt, and the moderator's reply becomes the
    /// card with `case.receipt.post_failed`.
    BecomePrivate,
    /// Mark the row `gone`. (Channel reconciliation, not the sync, turns a
    /// module off when its channel was deleted.)
    MarkGone,
}

/// Decides what happens to a missing message of `kind`. `current_channel`
/// is where it belongs now (the appeal, entry, queue, or honeypot channel
/// setting; `None` when unset). For a receipt, `create_refused` is whether
/// its first create was refused and `token_valid` whether the moderator's
/// token still works.
pub const fn on_missing(
    kind: MessageKind,
    current_channel: Option<ChannelId>,
    create_refused: bool,
    token_valid: bool,
) -> MissingPlan {
    match (kind, current_channel) {
        (MessageKind::CaseReceipt, _) if create_refused && token_valid => {
            MissingPlan::BecomePrivate
        }
        (MessageKind::CaseReceipt, _) => MissingPlan::MarkGone,
        (MessageKind::AppealPost, None) => MissingPlan::WaitForChannel,
        (_, Some(channel)) => MissingPlan::Repost(channel),
        (_, None) => MissingPlan::MarkGone,
    }
}

/// What happens to an earlier location in `stale` (§10.3 step 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaleCleanup {
    /// Edit it, without buttons, to `ticket.panel.moved` or
    /// `appeal.post.moved`.
    EditToMoved,
    /// Delete it (404 counts as done).
    Delete,
}

/// The cleanup for a stale location of `kind`: ticket panels and appeal
/// posts say where they moved; the honeypot warning, public receipts, and
/// ticket posts are deleted.
pub const fn stale_cleanup(kind: MessageKind) -> StaleCleanup {
    match kind {
        MessageKind::TicketPanel | MessageKind::AppealPost => StaleCleanup::EditToMoved,
        MessageKind::HoneypotWarning | MessageKind::CaseReceipt | MessageKind::TicketPost => {
            StaleCleanup::Delete
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn owners_and_names() {
        assert!(MessageKind::CaseReceipt.has_owner());
        assert!(MessageKind::TicketPost.has_owner());
        assert!(!MessageKind::TicketPanel.has_owner());
        assert!(!MessageKind::HoneypotWarning.has_owner());
        let names: Vec<&str> = MessageKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "case_receipt",
                "appeal_post",
                "ticket_panel",
                "ticket_post",
                "honeypot_warning"
            ]
        );
        let names: Vec<&str> = MessageState::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(names, ["active", "gone"]);
    }

    #[test]
    fn gone_is_final() {
        assert_eq!(MessageState::Active.mark_gone(), Ok(MessageState::Gone));
        assert!(MessageState::Gone.mark_gone().is_err());
    }

    #[test]
    fn routes() {
        use SyncRoute as R;
        let cases = [
            (true, true, true, false, R::EditOriginal),
            (true, true, true, true, R::EditOriginal),
            (true, false, true, true, R::MarkGone),
            (true, false, false, false, R::MarkGone),
            (false, true, false, false, R::Wait),
            (false, false, true, true, R::EditWithBot),
            (false, true, true, true, R::EditWithBot),
            (false, true, true, false, R::Create),
            (false, false, true, false, R::Create),
        ];
        for (private, token_valid, has_channel, posted, route) in cases {
            let row = SyncRow {
                private,
                token_valid,
                has_channel,
                posted,
            };
            assert_eq!(sync_route(row), route, "{row:?}");
        }
    }

    #[test]
    fn revisions_tokens_and_nonces() {
        assert!(needs_sync(2, 1));
        assert!(!needs_sync(1, 1));
        let at = datetime!(2026-11-17 12:00 UTC);
        assert_eq!(token_expires_at(at), Some(datetime!(2026-11-17 12:14 UTC)));
        let row = MessageRowId::from_uuid(Uuid::from_u128(0));
        assert_eq!(create_nonce(row, 1), "AAAAAAAAAAAAAAAAAAAAAA1");
        assert_eq!(create_nonce(row, 36), "AAAAAAAAAAAAAAAAAAAAAA10");
        assert_eq!(create_nonce(row, 46_655), "AAAAAAAAAAAAAAAAAAAAAAzzz");
        assert_eq!(create_nonce(row, 46_655).len(), 25);
        for generation in [1, 2, 1_000, 46_655, 46_656, u32::MAX] {
            assert!(create_nonce(row, generation).len() <= 25, "{generation}");
        }
        assert_ne!(create_nonce(row, 1), create_nonce(row, 2));
    }

    #[test]
    fn missing_handling() {
        use MessageKind as K;
        assert!(is_missing(Some(10_008)) && is_missing(Some(10_003)) && is_missing(Some(50_001)));
        assert!(!is_missing(Some(50_013)) && !is_missing(None));
        assert_eq!(sync_error_code(403, Some(50_001)), "http_403_50001");
        assert_eq!(sync_error_code(400, None), "http_400");
        let ch = ChannelId::new(9).unwrap();
        assert_eq!(
            on_missing(K::CaseReceipt, Some(ch), true, true),
            MissingPlan::BecomePrivate
        );
        assert_eq!(
            on_missing(K::CaseReceipt, Some(ch), true, false),
            MissingPlan::MarkGone
        );
        assert_eq!(
            on_missing(K::CaseReceipt, None, false, true),
            MissingPlan::MarkGone
        );
        assert_eq!(
            on_missing(K::AppealPost, Some(ch), false, false),
            MissingPlan::Repost(ch)
        );
        assert_eq!(
            on_missing(K::AppealPost, None, false, false),
            MissingPlan::WaitForChannel
        );
        for kind in [K::TicketPanel, K::TicketPost, K::HoneypotWarning] {
            assert_eq!(
                on_missing(kind, Some(ch), false, false),
                MissingPlan::Repost(ch)
            );
            assert_eq!(on_missing(kind, None, false, false), MissingPlan::MarkGone);
        }
        assert_eq!(stale_cleanup(K::TicketPanel), StaleCleanup::EditToMoved);
        assert_eq!(stale_cleanup(K::AppealPost), StaleCleanup::EditToMoved);
        for kind in [K::HoneypotWarning, K::CaseReceipt, K::TicketPost] {
            assert_eq!(stale_cleanup(kind), StaleCleanup::Delete);
        }
        assert_eq!(MAX_STALE, 5);
    }
}
