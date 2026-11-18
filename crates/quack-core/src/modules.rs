//! Domain types of the optional modules (§13): tickets, the server activity
//! log, and the honeypot, plus the channel settings they share.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::Duration;

use crate::ids::{ChannelId, RuleId, TicketId};
use crate::permissions::{MemberRefusalCode, Permission};
use crate::{InvalidTransition, UnknownValue};

str_enum! {
    /// An optional module.
    pub enum ModuleName {
        /// Private support threads.
        Tickets = "tickets",
        /// The server activity log.
        ActivityLog = "activity_log",
        /// The honeypot channel.
        Honeypot = "honeypot",
    }
}

str_enum! {
    /// Why Quack turned a module off itself (`module_disabled_reason`).
    pub enum ModuleDisabledReason {
        /// A channel it needs was deleted.
        ChannelDeleted = "channel_deleted",
    }
}

/// Why a module can't be turned on right now (§12.11 `module.problem.*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleProblem {
    /// No ticket entry channel.
    TicketEntryMissing,
    /// No ticket staff (queue) channel.
    TicketQueueMissing,
    /// The entry and staff channels are the same.
    TicketSameChannel,
    /// No honeypot channel.
    HoneypotChannelMissing,
    /// No honeypot rule.
    HoneypotRuleMissing,
    /// The honeypot rule has required fields.
    HoneypotRuleRequiredFields(RuleId),
    /// The activity log has no route.
    ActivityNoRoute,
    /// Quack lacks View Audit Log in the server.
    ActivityNeedsAuditLog,
    /// Quack lacks permissions in a channel.
    ChannelPermissions {
        /// The channel.
        channel: ChannelId,
        /// What Quack lacks there.
        missing: Vec<Permission>,
    },
    /// A staff channel is not staff-only.
    ChannelNotStaffOnly(ChannelId),
}

impl ModuleProblem {
    /// The problem's machine code (`module.problem.<code>`).
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TicketEntryMissing => "ticket_entry_missing",
            Self::TicketQueueMissing => "ticket_queue_missing",
            Self::TicketSameChannel => "ticket_same_channel",
            Self::HoneypotChannelMissing => "honeypot_channel_missing",
            Self::HoneypotRuleMissing => "honeypot_rule_missing",
            Self::HoneypotRuleRequiredFields(_) => "honeypot_rule_required_fields",
            Self::ActivityNoRoute => "activity_no_route",
            Self::ActivityNeedsAuditLog => "activity_needs_audit_log",
            Self::ChannelPermissions { .. } => "channel_permissions",
            Self::ChannelNotStaffOnly(_) => "channel_not_staff_only",
        }
    }
}

str_enum! {
    /// A channel setting cleared because its channel is gone.
    pub enum ChannelSetting {
        /// `log_channel_id`.
        LogChannel = "log_channel",
        /// `appeal_channel_id`.
        AppealChannel = "appeal_channel",
        /// `evidence_channel_id`.
        EvidenceChannel = "evidence_channel",
        /// The ticket entry channel.
        TicketEntryChannel = "ticket_entry_channel",
        /// The ticket staff channel.
        TicketQueueChannel = "ticket_queue_channel",
        /// The honeypot channel.
        HoneypotChannel = "honeypot_channel",
        /// A server activity log route.
        ActivityLogRoute = "activity_log_route",
    }
}

str_enum! {
    /// Why a log channel post was dropped.
    pub enum LogChannelFailure {
        /// The log channel is no longer staff-only (or Quack lost access).
        NotStaffOnly = "not_staff_only",
        /// Discord refused the message.
        DiscordRefused = "discord_refused",
        /// The retries ran out.
        RetriesExhausted = "retries_exhausted",
    }
}

str_enum! {
    /// A Discord event the server activity log can post (`activity_event`).
    pub enum ActivityEvent {
        /// A message's text or files changed.
        MessageEdited = "message_edited",
        /// A message was deleted.
        MessageDeleted = "message_deleted",
        /// Messages were bulk deleted.
        MessagesBulkDeleted = "messages_bulk_deleted",
        /// A member joined.
        MemberJoined = "member_joined",
        /// A member left.
        MemberLeft = "member_left",
        /// Someone other than Quack banned a member.
        MemberBanned = "member_banned",
        /// Someone other than Quack unbanned a member.
        MemberUnbanned = "member_unbanned",
        /// The server's settings changed.
        ServerChanged = "server_changed",
        /// A channel was created, changed, or deleted.
        ChannelChanged = "channel_changed",
    }
}

str_enum! {
    /// A ticket's state (`ticket_state`).
    pub enum TicketState {
        /// Its thread is being created.
        Opening = "opening",
        /// The member and staff can talk.
        Open = "open",
        /// The close job is running; it always finishes.
        Closing = "closing",
        /// Done; the member may open another.
        Closed = "closed",
    }
}

str_enum! {
    /// Why a ticket closed without opening (`ticket_close_reason`).
    pub enum TicketCloseReason {
        /// Discord refused to create the thread.
        OpenFailed = "open_failed",
    }
}

/// An event in the ticket transition table (§13.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketEvent {
    /// Thread created, member added, welcome posted.
    Opened,
    /// Thread creation was refused.
    OpenFailed,
    /// The member or a moderator closed it, or its thread was deleted.
    CloseRequested,
    /// The close job finished or ran out of budget.
    CloseFinished,
}

impl TicketState {
    /// Whether the ticket holds the member's one live slot.
    pub const fn is_live(self) -> bool {
        !matches!(self, Self::Closed)
    }

    /// Applies one event (§13.2.2).
    pub fn transition(self, event: TicketEvent) -> Result<Self, InvalidTransition> {
        match (self, event) {
            (Self::Opening, TicketEvent::Opened) => Ok(Self::Open),
            (Self::Opening, TicketEvent::OpenFailed)
            | (Self::Closing, TicketEvent::CloseFinished) => Ok(Self::Closed),
            (Self::Open, TicketEvent::CloseRequested) => Ok(Self::Closing),
            _ => Err(InvalidTransition {
                machine: "ticket",
                from: self.as_str(),
                event: match event {
                    TicketEvent::Opened => "opened",
                    TicketEvent::OpenFailed => "open_failed",
                    TicketEvent::CloseRequested => "close_requested",
                    TicketEvent::CloseFinished => "close_finished",
                },
            }),
        }
    }
}

/// The ticket's short id: the first 10 characters of its u22, used in the
/// thread name `ticket-<short_id>`.
pub fn ticket_short_id(id: TicketId) -> String {
    id.to_u22().chars().take(10).collect()
}

str_enum! {
    /// A honeypot hit's state (`honeypot_hit_state`).
    pub enum HoneypotHitState {
        /// Waiting for the `honeypot_case` job.
        Pending = "pending",
        /// Attached to a case.
        CaseCreated = "case_created",
        /// No case: a refusal or no usable rule.
        Failed = "failed",
    }
}

impl HoneypotHitState {
    /// Settles a pending hit; a settled hit never changes.
    pub fn settle(self, case_created: bool) -> Result<Self, InvalidTransition> {
        match self {
            Self::Pending if case_created => Ok(Self::CaseCreated),
            Self::Pending => Ok(Self::Failed),
            _ => Err(InvalidTransition {
                machine: "honeypot_hit",
                from: self.as_str(),
                event: if case_created {
                    "case_created"
                } else {
                    "failed"
                },
            }),
        }
    }
}

/// A burst of posts within this long after a honeypot case joins that case.
pub const HONEYPOT_BURST_WINDOW: Duration = Duration::seconds(30);

/// Why the honeypot couldn't add a case: a member refusal code or
/// `rule_unavailable`. Stored as text in `honeypot_hits.failure`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoneypotFailure {
    /// A member check refused.
    Member(MemberRefusalCode),
    /// The honeypot's rule is missing or archived.
    RuleUnavailable,
}

impl HoneypotFailure {
    /// The stored code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member(code) => code.as_str(),
            Self::RuleUnavailable => "rule_unavailable",
        }
    }
}

impl std::str::FromStr for HoneypotFailure {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        if s == "rule_unavailable" {
            return Ok(Self::RuleUnavailable);
        }
        s.parse().map(Self::Member).map_err(|_| UnknownValue {
            type_name: "HoneypotFailure",
            value: s.to_owned(),
        })
    }
}

impl Serialize for HoneypotFailure {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HoneypotFailure {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn ticket_transitions() {
        use TicketEvent as E;
        use TicketState as S;
        let events = [
            E::Opened,
            E::OpenFailed,
            E::CloseRequested,
            E::CloseFinished,
        ];
        for &state in S::ALL {
            for event in events {
                let expected = match (state, event) {
                    (S::Opening, E::Opened) => Some(S::Open),
                    (S::Opening, E::OpenFailed) | (S::Closing, E::CloseFinished) => Some(S::Closed),
                    (S::Open, E::CloseRequested) => Some(S::Closing),
                    _ => None,
                };
                assert_eq!(
                    state.transition(event).ok(),
                    expected,
                    "{state:?} {event:?}"
                );
            }
        }
        assert!(S::Closing.is_live());
        assert!(!S::Closed.is_live());
    }

    #[test]
    fn hit_settles_once() {
        assert_eq!(
            HoneypotHitState::Pending.settle(true),
            Ok(HoneypotHitState::CaseCreated)
        );
        assert_eq!(
            HoneypotHitState::Pending.settle(false),
            Ok(HoneypotHitState::Failed)
        );
        assert!(HoneypotHitState::Failed.settle(true).is_err());
        assert!(HoneypotHitState::CaseCreated.settle(false).is_err());
    }

    #[test]
    fn honeypot_failure_codes() {
        for s in ["rule_unavailable", "member_is_bot", "quack_role_too_low"] {
            let f: HoneypotFailure = s.parse().unwrap();
            assert_eq!(f.as_str(), s);
            assert_eq!(
                serde_json::to_value(f).unwrap(),
                serde_json::Value::String(s.into())
            );
        }
        assert!("exempt".parse::<HoneypotFailure>().is_err());
    }

    #[test]
    fn short_id_and_codes() {
        let id = TicketId::from_uuid(Uuid::from_u128(u128::MAX));
        assert_eq!(ticket_short_id(id), "__________");
        assert_eq!(
            ModuleProblem::ChannelNotStaffOnly(ChannelId::new(1).unwrap()).code(),
            "channel_not_staff_only"
        );
        assert_eq!(
            ModuleProblem::TicketSameChannel.code(),
            "ticket_same_channel"
        );
    }
}
