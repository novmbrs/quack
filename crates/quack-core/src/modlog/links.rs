//! Which typed links each moderation log kind carries (§9.2).

use super::ModLogKind;
use crate::ids::{ActionId, AppealId, CaseId, MemberId, RuleId, TicketId};

/// An entry's typed link columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModLogLinks {
    /// The case.
    pub case_id: Option<CaseId>,
    /// The member.
    pub member_id: Option<MemberId>,
    /// The appeal.
    pub appeal_id: Option<AppealId>,
    /// The action.
    pub action_id: Option<ActionId>,
    /// The rule.
    pub rule_id: Option<RuleId>,
    /// The ticket.
    pub ticket_id: Option<TicketId>,
}

/// One link column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// `case_id`.
    Case,
    /// `member_id`.
    Member,
    /// `appeal_id`.
    Appeal,
    /// `action_id`.
    Action,
    /// `rule_id`.
    Rule,
    /// `ticket_id`.
    Ticket,
}

/// Whether a kind carries a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkRule {
    /// Always set.
    Required,
    /// Set when it applies.
    Optional,
    /// Never set.
    Absent,
}

/// A link that is missing or should not be there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkProblem {
    /// A required link is not set.
    #[error("{kind:?} entries need a {link:?} link")]
    Missing {
        /// The entry's kind.
        kind: ModLogKind,
        /// The missing link.
        link: Link,
    },
    /// A link the kind never carries is set.
    #[error("{kind:?} entries never carry a {link:?} link")]
    Unexpected {
        /// The entry's kind.
        kind: ModLogKind,
        /// The unexpected link.
        link: Link,
    },
}

impl ModLogKind {
    /// Whether entries of this kind carry `link`.
    pub const fn link_rule(self, link: Link) -> LinkRule {
        use LinkRule::{Absent, Optional, Required};
        use ModLogKind as K;
        let [case, member, appeal, action, rule, ticket] = match self {
            K::CaseCreated => [Required, Required, Absent, Absent, Required, Absent],
            K::CaseCreateRefused => [Absent, Required, Absent, Absent, Required, Absent],
            K::CaseVoided => [Required, Required, Optional, Absent, Absent, Absent],
            K::CaseContextEdited | K::EvidenceAdded => {
                [Required, Required, Absent, Absent, Absent, Absent]
            }
            K::ActionDone
            | K::ActionFailed
            | K::ActionRetryRequested
            | K::ActionDismissed
            | K::ActionUndoRequested
            | K::ActionUndoSkipped => [Required, Required, Absent, Required, Absent, Absent],
            K::DmSent | K::DmFailed => [Optional, Required, Optional, Absent, Absent, Optional],
            K::AppealSubmitted
            | K::AppealInformationRequested
            | K::AppealInformationProvided
            | K::AppealAccepted
            | K::AppealRejected
            | K::AppealClosed
            | K::AppealReopened => [Required, Required, Required, Absent, Absent, Absent],
            K::RuleCreated
            | K::RuleEdited
            | K::RuleArchived
            | K::RuleRestored
            | K::RuleImported => [Absent, Absent, Absent, Absent, Required, Absent],
            K::QuackJoined => [Absent, Absent, Absent, Absent, Optional, Absent],
            K::TicketOpened | K::TicketClosed => {
                [Absent, Required, Absent, Absent, Absent, Required]
            }
            K::HoneypotCaseFailed => [Absent, Required, Absent, Absent, Optional, Absent],
            K::Legacy => [Optional; 6],
            K::SettingsChanged
            | K::ModuleEnabled
            | K::ModuleDisabled
            | K::ModuleSettingsChanged
            | K::QuackLeft
            | K::ChannelSettingCleared
            | K::LogChannelFailed => [Absent; 6],
        };
        match link {
            Link::Case => case,
            Link::Member => member,
            Link::Appeal => appeal,
            Link::Action => action,
            Link::Rule => rule,
            Link::Ticket => ticket,
        }
    }
}

impl ModLogLinks {
    fn is_set(&self, link: Link) -> bool {
        match link {
            Link::Case => self.case_id.is_some(),
            Link::Member => self.member_id.is_some(),
            Link::Appeal => self.appeal_id.is_some(),
            Link::Action => self.action_id.is_some(),
            Link::Rule => self.rule_id.is_some(),
            Link::Ticket => self.ticket_id.is_some(),
        }
    }

    /// Checks the links against the kind's rules before writing an entry.
    pub fn check(&self, kind: ModLogKind) -> Result<(), LinkProblem> {
        for link in [
            Link::Case,
            Link::Member,
            Link::Appeal,
            Link::Action,
            Link::Rule,
            Link::Ticket,
        ] {
            match (kind.link_rule(link), self.is_set(link)) {
                (LinkRule::Required, false) => return Err(LinkProblem::Missing { kind, link }),
                (LinkRule::Absent, true) => return Err(LinkProblem::Unexpected { kind, link }),
                _ => {}
            }
        }
        Ok(())
    }
}
