//! Domain types of the optional modules (§13): tickets, the server activity
//! log, and the honeypot, plus the channel settings they share.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
        /// Quack left the server.
        QuackLeft = "quack_left",
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
        /// A step of opening was refused, the thread was deleted, or the
        /// open job's budget ran out.
        OpenFailed = "open_failed",
    }
}

str_enum! {
    /// Why a ticket transcript is partial (`transcript_partial`).
    pub enum TranscriptPartial {
        /// The thread's history was unavailable or didn't reach its first
        /// message, and the journal has gaps.
        History = "history",
        /// It was cut to 8 MiB.
        Trimmed = "trimmed",
    }
}

/// An event in the ticket transition table (§13.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketEvent {
    /// Thread ready, member added, welcome posted.
    Opened,
    /// A step was definitely refused, the thread was deleted, or the open
    /// job's budget is spent.
    OpenFailed {
        /// Whether a thread exists (it must be deleted by the close job).
        thread_exists: bool,
    },
    /// The member or a moderator closed it, or its thread was deleted.
    CloseRequested,
    /// The close job settled or ran out of budget.
    CloseFinished,
}

impl TicketEvent {
    /// The event's name, for errors and logs.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::OpenFailed { .. } => "open_failed",
            Self::CloseRequested => "close_requested",
            Self::CloseFinished => "close_finished",
        }
    }
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
            (
                Self::Opening,
                TicketEvent::OpenFailed {
                    thread_exists: false,
                },
            )
            | (Self::Closing, TicketEvent::CloseFinished) => Ok(Self::Closed),
            (
                Self::Opening,
                TicketEvent::OpenFailed {
                    thread_exists: true,
                },
            )
            | (Self::Open, TicketEvent::CloseRequested) => Ok(Self::Closing),
            _ => Err(InvalidTransition {
                machine: "ticket",
                from: self.as_str(),
                event: event.name(),
            }),
        }
    }
}

/// A ticket thread's name, `ticket-<ticket u22>`: the full id, so names
/// never collide and a lost create can be found again (§13.2.3).
pub fn ticket_thread_name(id: TicketId) -> String {
    format!("ticket-{}", id.to_u22())
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

/// Why the honeypot couldn't add a case, as `honeypot_case_failed`
/// records it (§9.2, §13.4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoneypotRefusal {
    /// A member check refused.
    Member(MemberRefusalCode),
    /// The honeypot's rule is missing or archived.
    RuleUnavailable,
    /// The honeypot's settings changed after the hits were admitted.
    PolicyChanged,
    /// Discord stayed unavailable until the job's budget ran out.
    DiscordUnavailable,
}

impl HoneypotRefusal {
    /// The stored code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member(code) => code.as_str(),
            Self::RuleUnavailable => "rule_unavailable",
            Self::PolicyChanged => "policy_changed",
            Self::DiscordUnavailable => "discord_unavailable",
        }
    }
}

/// Why a honeypot hit `failed` (`honeypot_hits.failure`): a refusal, or a
/// hit that never became a case for another reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoneypotFailure {
    /// The case was refused (also logged).
    Refused(HoneypotRefusal),
    /// The poster is staff for Discord access, or a bot, by current facts.
    Exempt,
    /// A hit the Go version left pending at cutover (§19.9).
    ImportedPending,
}

impl HoneypotFailure {
    /// The stored code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Refused(refusal) => refusal.as_str(),
            Self::Exempt => "exempt",
            Self::ImportedPending => "imported_pending",
        }
    }
}

impl std::str::FromStr for HoneypotRefusal {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        match s {
            "rule_unavailable" => Ok(Self::RuleUnavailable),
            "policy_changed" => Ok(Self::PolicyChanged),
            "discord_unavailable" => Ok(Self::DiscordUnavailable),
            _ => s.parse().map(Self::Member).map_err(|_| UnknownValue {
                type_name: "HoneypotRefusal",
                value: s.to_owned(),
            }),
        }
    }
}

impl std::str::FromStr for HoneypotFailure {
    type Err = UnknownValue;

    fn from_str(s: &str) -> Result<Self, UnknownValue> {
        match s {
            "exempt" => Ok(Self::Exempt),
            "imported_pending" => Ok(Self::ImportedPending),
            _ => s.parse().map(Self::Refused).map_err(|_| UnknownValue {
                type_name: "HoneypotFailure",
                value: s.to_owned(),
            }),
        }
    }
}

macro_rules! string_serde {
    ($($ty:ty),+) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                String::deserialize(d)?
                    .parse()
                    .map_err(serde::de::Error::custom)
            }
        }
    )+};
}

string_serde!(HoneypotRefusal, HoneypotFailure);

/// What replaces a secret in server activity text.
pub const REDACTED_SECRET: &str = "[redacted]";

/// Replaces secrets in text the server activity log caches or posts
/// (§13.3.2), the Go patterns plus `password` and `bearer`: Discord bot
/// tokens (bare, or after `Bot `), webhook URLs, and the values after
/// `token`, `secret`, `password`, or `authorization` followed by `=` or
/// `:` (an authorization value runs to the end of the line), and after
/// `Bearer `. Matching ignores case; the key is kept and
/// only its value becomes `[redacted]`. Text without secrets is borrowed
/// unchanged.
pub fn redact_secrets(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < text.len() {
        if let Some((keep, end)) = secret_at(text, i) {
            out.push_str(&text[copied..keep]);
            out.push_str(REDACTED_SECRET);
            copied = end;
            i = end;
        } else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if copied == 0 {
        Cow::Borrowed(text)
    } else {
        out.push_str(&text[copied..]);
        Cow::Owned(out)
    }
}

/// When a secret starts at byte `i`: where its redacted part starts (the
/// prefix before it is kept) and where it ends.
fn secret_at(text: &str, i: usize) -> Option<(usize, usize)> {
    let rest = &text[i..];
    let boundary = text[..i]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    for key in ["token", "secret", "password", "authorization"] {
        if starts_with_ignore_case(rest, key) {
            let after = skip_spaces(text, i + key.len());
            if let Some(sep) = text[after..]
                .chars()
                .next()
                .filter(|c| *c == '=' || *c == ':')
            {
                let value = skip_spaces(text, after + sep.len_utf8());
                // An authorization value is a scheme and a credential, so
                // it runs to the end of the line.
                let end = if key == "authorization" {
                    value + text[value..].find('\n').unwrap_or(text.len() - value)
                } else {
                    value_end(text, value)
                };
                if end > value {
                    return Some((value, end));
                }
            }
        }
    }
    if boundary {
        for (key, min_len) in [("bot", 20), ("bearer", 1)] {
            if starts_with_ignore_case(rest, key) {
                let value = skip_spaces(text, i + key.len());
                let end = value_end(text, value);
                let token_like = text[value..end]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
                if value > i + key.len()
                    && end - value >= min_len
                    && (key == "bearer" || token_like)
                {
                    return Some((value, end));
                }
            }
        }
        if let Some(end) = bare_token_end(text, i) {
            return Some((i, end));
        }
    }
    if starts_with_ignore_case(rest, "https://") {
        let path = &text[i + "https://".len()..];
        let host = [
            "discord.com/api/",
            "discordapp.com/api/",
            "ptb.discord.com/api/",
            "canary.discord.com/api/",
        ]
        .iter()
        .find(|h| starts_with_ignore_case(path, h))
        .map_or(0, |h| h.len());
        let mut at = i + "https://".len() + host;
        if host > 0 {
            // An optional API version segment, e.g. `v10/`.
            let segment = &text[at..];
            if let Some(slash) = segment.find('/')
                && segment.starts_with('v')
                && slash > 1
                && segment[1..slash].bytes().all(|b| b.is_ascii_digit())
            {
                at += slash + 1;
            }
        }
        if starts_with_ignore_case(&text[at..], "webhooks/") {
            let end = value_end(text, at + "webhooks/".len());
            if end > at + "webhooks/".len() {
                return Some((i, end));
            }
        }
    }
    None
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn skip_spaces(text: &str, from: usize) -> usize {
    from + text[from..]
        .find(|c: char| !c.is_whitespace())
        .unwrap_or(text.len() - from)
}

fn value_end(text: &str, from: usize) -> usize {
    from + text[from..]
        .find(char::is_whitespace)
        .unwrap_or(text.len() - from)
}

/// A bare Discord bot token at byte `i`: three base64url parts joined by
/// dots (at least 23, 6, and 27 characters), ending at a non-token
/// character.
fn bare_token_end(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let part = |from: usize| {
        from + bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
            .count()
    };
    let first = part(i);
    if first - i < 23 || bytes.get(first) != Some(&b'.') {
        return None;
    }
    let second = part(first + 1);
    if !(6..=7).contains(&(second - first - 1)) || bytes.get(second) != Some(&b'.') {
        return None;
    }
    let third = part(second + 1);
    (third - second > 27).then_some(third)
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
            E::OpenFailed {
                thread_exists: false,
            },
            E::OpenFailed {
                thread_exists: true,
            },
            E::CloseRequested,
            E::CloseFinished,
        ];
        for &state in S::ALL {
            for event in events {
                let expected = match (state, event) {
                    (S::Opening, E::Opened) => Some(S::Open),
                    (
                        S::Opening,
                        E::OpenFailed {
                            thread_exists: false,
                        },
                    )
                    | (S::Closing, E::CloseFinished) => Some(S::Closed),
                    (
                        S::Opening,
                        E::OpenFailed {
                            thread_exists: true,
                        },
                    )
                    | (S::Open, E::CloseRequested) => Some(S::Closing),
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
        for s in [
            "rule_unavailable",
            "policy_changed",
            "discord_unavailable",
            "member_is_bot",
            "quack_role_too_low",
        ] {
            let r: HoneypotRefusal = s.parse().unwrap();
            assert_eq!(r.as_str(), s);
            let f: HoneypotFailure = s.parse().unwrap();
            assert_eq!(f, HoneypotFailure::Refused(r));
            assert_eq!(
                serde_json::to_value(r).unwrap(),
                serde_json::Value::String(s.into())
            );
        }
        for s in ["exempt", "imported_pending"] {
            let f: HoneypotFailure = s.parse().unwrap();
            assert_eq!(f.as_str(), s);
            assert!(s.parse::<HoneypotRefusal>().is_err());
        }
        assert!("nope".parse::<HoneypotFailure>().is_err());
    }

    #[test]
    fn enum_names_match_the_database() {
        let names = |v: &[&str]| v.join(",");
        assert_eq!(
            names(
                &ModuleDisabledReason::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            ),
            "channel_deleted,quack_left"
        );
        assert_eq!(
            names(
                &TranscriptPartial::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            ),
            "history,trimmed"
        );
        assert_eq!(
            names(
                &TicketState::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            ),
            "opening,open,closing,closed"
        );
        assert_eq!(
            names(
                &ActivityEvent::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            ),
            "message_edited,message_deleted,messages_bulk_deleted,member_joined,member_left,\
             member_banned,member_unbanned,server_changed,channel_changed"
        );
        assert_eq!(
            names(
                &HoneypotHitState::ALL
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            ),
            "pending,case_created,failed"
        );
    }

    #[test]
    fn redacting_secrets() {
        let token = format!("{}.{}.{}", "M".repeat(24), "Gabcde", "x".repeat(38));
        let cases = [
            ("nothing to see", "nothing to see"),
            ("token=abc123 rest", "token=[redacted] rest"),
            ("my TOKEN : abc rest", "my TOKEN : [redacted] rest"),
            ("password=hunter2", "password=[redacted]"),
            ("Secret:xyz", "Secret:[redacted]"),
            (
                "Authorization: Bot abc\nnext",
                "Authorization: [redacted]\nnext",
            ),
            ("Bearer abc.def", "Bearer [redacted]"),
            ("bot abcdefghijklmnopqrstuvwxyz", "bot [redacted]"),
            ("bot short", "bot short"),
            (
                "robot abcdefghijklmnopqrstuvwxyz",
                "robot abcdefghijklmnopqrstuvwxyz",
            ),
            ("token=", "token="),
            (
                "hook https://discord.com/api/webhooks/1/abc end",
                "hook [redacted] end",
            ),
            (
                "https://canary.discord.com/api/v10/webhooks/1/abc",
                "[redacted]",
            ),
            ("https://discordapp.com/api/webhooks/1/x", "[redacted]"),
            (
                "https://example.com/webhooks",
                "https://example.com/webhooks",
            ),
            ("é token=ü é", "é token=[redacted] é"),
        ];
        for (input, want) in cases {
            assert_eq!(redact_secrets(input), want, "{input}");
        }
        assert_eq!(
            redact_secrets(&format!("leaked {token} oops")),
            "leaked [redacted] oops"
        );
        assert!(matches!(redact_secrets("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn thread_name_and_codes() {
        let id = TicketId::from_uuid(Uuid::from_u128(u128::MAX));
        assert_eq!(ticket_thread_name(id), format!("ticket-{}", id.to_u22()));
        assert_eq!(ticket_thread_name(id).len(), 29);
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
