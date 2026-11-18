//! Cases (§4): sources, the stored decision, validity and voiding,
//! replacement links, and case context.

mod context;
mod link;

use crate::DomainError;
use crate::action::ActionKind;
use crate::escalation::{LevelChoice, select_level};
use crate::ids::RuleId;
use crate::rule::{LevelAction, RuleDefinition};

pub use context::{
    ContextInput, ContextProblem, ContextProblemCode, ContextValue, ContextValues,
    fill_message_links, validate_context,
};
pub use link::{LinkProblem, MessageLink};

str_enum! {
    /// Where a case came from (`case_source`).
    pub enum CaseSource {
        /// `/case add` or an "Add case" command.
        Discord = "discord",
        /// The dashboard.
        Dashboard = "dashboard",
        /// The honeypot module, with Quack as the actor.
        Honeypot = "honeypot",
        /// A Quack v4 case the old system imported; never counted, no rule,
        /// no action, no DM, no appeal.
        Imported = "imported",
    }
}

str_enum! {
    /// Why a case was voided (`void_cause`).
    pub enum VoidCause {
        /// A moderator voided it.
        Staff = "staff",
        /// Accepting its appeal voided it.
        AppealAccepted = "appeal_accepted",
    }
}

/// Whether a case still counts. There is no un-void.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseState {
    /// Counts toward escalation; its actions may run.
    Valid,
    /// Kept but no longer counted.
    Voided,
}

impl CaseState {
    /// The state from the stored `voided_at` column.
    pub const fn from_voided(voided: bool) -> Self {
        if voided { Self::Voided } else { Self::Valid }
    }

    /// Voids a valid case (staff void or an accepted appeal); a voided case
    /// refuses with `case_already_voided` (§4.5).
    pub const fn void(self) -> Result<Self, DomainError> {
        match self {
            Self::Valid => Ok(Self::Voided),
            Self::Voided => Err(DomainError::CaseAlreadyVoided),
        }
    }
}

/// The decision stored on a case: exactly what was chosen, never changed.
/// Corrections are a void plus a replacement case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseDecision {
    /// The rule applied.
    pub rule_id: RuleId,
    /// The immutable version applied.
    pub rule_version: u32,
    /// Counted cases including this one.
    pub case_count: u32,
    /// 1-based level position in that version.
    pub level_index: u8,
    /// That level's `from_case_count`.
    pub level_from_case_count: u16,
    /// The level's action; `None` is a warning.
    pub action: Option<LevelAction>,
    /// Whether the member gets a case DM.
    pub sends_dm: bool,
}

impl CaseDecision {
    /// Decides a new case under `def` (the rule's current version) given
    /// how many past cases count (§4.2 steps 10–11). `None` only for a
    /// definition without levels.
    pub fn decide(
        rule_id: RuleId,
        rule_version: u32,
        def: &RuleDefinition,
        counted_cases: u32,
    ) -> Option<Self> {
        let case_count = counted_cases.saturating_add(1);
        select_level(def, case_count)
            .map(|choice| Self::from_choice(rule_id, rule_version, case_count, choice))
    }

    /// Builds the decision from a level choice.
    pub const fn from_choice(
        rule_id: RuleId,
        rule_version: u32,
        case_count: u32,
        choice: LevelChoice,
    ) -> Self {
        Self {
            rule_id,
            rule_version,
            case_count,
            level_index: choice.index,
            level_from_case_count: choice.from_case_count,
            action: choice.action,
            sends_dm: choice.send_dm,
        }
    }

    /// The `action_kind` column.
    pub fn action_kind(&self) -> Option<ActionKind> {
        self.action.map(LevelAction::kind)
    }

    /// The `timeout_seconds` column.
    pub const fn timeout_seconds(&self) -> Option<u32> {
        match self.action {
            Some(LevelAction::Timeout { duration_seconds }) => Some(duration_seconds),
            _ => None,
        }
    }

    /// The `ban_delete_message_seconds` column.
    pub const fn ban_delete_message_seconds(&self) -> Option<u32> {
        match self.action {
            Some(LevelAction::Ban {
                delete_message_seconds,
            }) => Some(delete_message_seconds),
            _ => None,
        }
    }
}

/// Checks a case named in `replaces_case_id` (§4.8): it must be voided and
/// not already replaced. (Same server is guaranteed by looking it up there.)
pub const fn check_replacement(voided: bool, already_replaced: bool) -> Result<(), DomainError> {
    if voided && !already_replaced {
        Ok(())
    } else {
        Err(DomainError::ReplacementInvalid)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::rule::starter_rule;

    #[test]
    fn void_is_one_way() {
        assert_eq!(CaseState::Valid.void(), Ok(CaseState::Voided));
        assert_eq!(
            CaseState::Voided.void(),
            Err(DomainError::CaseAlreadyVoided)
        );
        assert_eq!(CaseState::from_voided(true), CaseState::Voided);
        assert_eq!(CaseState::from_voided(false), CaseState::Valid);
    }

    #[test]
    fn decision_columns() {
        let rule = RuleId::from_uuid(Uuid::from_u128(1));
        let def = starter_rule();
        let first = CaseDecision::decide(rule, 2, &def, 0).unwrap();
        assert_eq!(
            (first.case_count, first.level_index, first.action_kind()),
            (1, 1, None)
        );
        assert!(first.sends_dm);
        let third = CaseDecision::decide(rule, 2, &def, 2).unwrap();
        assert_eq!(
            (
                third.case_count,
                third.level_index,
                third.level_from_case_count
            ),
            (3, 2, 3)
        );
        assert_eq!(
            (
                third.action_kind(),
                third.timeout_seconds(),
                third.ban_delete_message_seconds()
            ),
            (Some(ActionKind::Timeout), Some(86_400), None)
        );
        let fifth = CaseDecision::decide(rule, 2, &def, 40).unwrap();
        assert_eq!(
            (
                fifth.action_kind(),
                fifth.ban_delete_message_seconds(),
                fifth.timeout_seconds()
            ),
            (Some(ActionKind::Ban), Some(86_400), None)
        );
        assert_eq!(
            CaseDecision::decide(rule, 1, &def, u32::MAX)
                .unwrap()
                .case_count,
            u32::MAX
        );
    }

    #[test]
    fn replacement_rules() {
        assert_eq!(check_replacement(true, false), Ok(()));
        assert_eq!(
            check_replacement(false, false),
            Err(DomainError::ReplacementInvalid)
        );
        assert_eq!(
            check_replacement(true, true),
            Err(DomainError::ReplacementInvalid)
        );
    }

    #[test]
    fn enum_names() {
        assert_eq!(CaseSource::Imported.as_str(), "imported");
        assert_eq!(
            "appeal_accepted".parse::<VoidCause>(),
            Ok(VoidCause::AppealAccepted)
        );
        assert!("deleted".parse::<VoidCause>().is_err());
    }
}
