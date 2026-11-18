//! Rule definition validation (§3.3).

use std::collections::HashSet;
use std::fmt;

use super::{LevelAction, RuleDefinition};
use crate::limits;

/// How strictly a definition is validated (§3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strictness {
    /// Every row of §3.3: every definition written through Quack (create,
    /// edit, import of a rule file).
    New,
    /// Only for the MySQL importer (§19.5): structure, not size limits, so
    /// legacy rules are imported as they were. Checks a non-empty name and
    /// reason, 1 or more levels with strictly increasing `from_case_count`,
    /// timeouts within Discord's 28 days, and unique field keys.
    Imported {
        /// A partial version rebuilt from old cases, never applied to a
        /// new case: its first level may start above 1.
        reconstructed: bool,
    },
}

str_enum! {
    /// The machine code of one validation problem, as the API reports it.
    pub enum RuleProblemCode {
        /// `name` is not 1–100 characters.
        NameLength = "name_length",
        /// `description` is over 500 characters.
        DescriptionLength = "description_length",
        /// `reason` is not 1–1,000 characters.
        ReasonLength = "reason_length",
        /// `counting_window_days` is not null or 1–36,500.
        CountingWindow = "counting_window",
        /// More than 10 fields.
        TooManyFields = "too_many_fields",
        /// A field key does not match the pattern.
        FieldKey = "field_key",
        /// A field key repeats an earlier one.
        FieldKeyDuplicate = "field_key_duplicate",
        /// A field label is not 1–100 characters.
        FieldLabelLength = "field_label_length",
        /// Not 1–10 levels.
        LevelCount = "level_count",
        /// The first level does not start at case count 1.
        FirstLevelNotOne = "first_level_not_one",
        /// A later level starts outside 2–1,000,000.
        LevelCaseCount = "level_case_count",
        /// A level does not start after the one before it.
        LevelOrder = "level_order",
        /// A timeout is not 1–2,419,200 seconds.
        TimeoutDuration = "timeout_duration",
        /// A ban deletes more than 604,800 seconds of messages.
        BanDeleteWindow = "ban_delete_window",
    }
}

/// One problem with a rule definition: which field, and what is wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleProblem {
    /// `name` is not 1–100 characters.
    NameLength,
    /// `description` is over 500 characters.
    DescriptionLength,
    /// `reason` is not 1–1,000 characters.
    ReasonLength,
    /// `counting_window_days` is out of range.
    CountingWindow,
    /// More than 10 fields.
    TooManyFields,
    /// `fields[index].key` does not match the pattern.
    FieldKey {
        /// Position of the field.
        index: usize,
    },
    /// `fields[index].key` repeats an earlier key.
    FieldKeyDuplicate {
        /// Position of the repeated field.
        index: usize,
    },
    /// `fields[index].label` is not 1–100 characters.
    FieldLabelLength {
        /// Position of the field.
        index: usize,
    },
    /// Not 1–10 levels.
    LevelCount,
    /// `levels[0].from_case_count` is not 1.
    FirstLevelNotOne,
    /// `levels[index].from_case_count` is outside its range.
    LevelCaseCount {
        /// Position of the level.
        index: usize,
    },
    /// `levels[index].from_case_count` is not above the previous level's.
    LevelOrder {
        /// Position of the level.
        index: usize,
    },
    /// `levels[index].action.duration_seconds` is out of range.
    TimeoutDuration {
        /// Position of the level.
        index: usize,
    },
    /// `levels[index].action.delete_message_seconds` is out of range.
    BanDeleteWindow {
        /// Position of the level.
        index: usize,
    },
}

impl RuleProblem {
    /// The problem's machine code.
    pub const fn code(self) -> RuleProblemCode {
        match self {
            Self::NameLength => RuleProblemCode::NameLength,
            Self::DescriptionLength => RuleProblemCode::DescriptionLength,
            Self::ReasonLength => RuleProblemCode::ReasonLength,
            Self::CountingWindow => RuleProblemCode::CountingWindow,
            Self::TooManyFields => RuleProblemCode::TooManyFields,
            Self::FieldKey { .. } => RuleProblemCode::FieldKey,
            Self::FieldKeyDuplicate { .. } => RuleProblemCode::FieldKeyDuplicate,
            Self::FieldLabelLength { .. } => RuleProblemCode::FieldLabelLength,
            Self::LevelCount => RuleProblemCode::LevelCount,
            Self::FirstLevelNotOne => RuleProblemCode::FirstLevelNotOne,
            Self::LevelCaseCount { .. } => RuleProblemCode::LevelCaseCount,
            Self::LevelOrder { .. } => RuleProblemCode::LevelOrder,
            Self::TimeoutDuration { .. } => RuleProblemCode::TimeoutDuration,
            Self::BanDeleteWindow { .. } => RuleProblemCode::BanDeleteWindow,
        }
    }

    /// The JSON path of the offending field, e.g. `levels[1].from_case_count`,
    /// as the API's `fields[].field` reports it.
    pub fn path(self) -> String {
        self.to_string()
    }
}

impl fmt::Display for RuleProblem {
    /// Writes the problem's JSON path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NameLength => f.write_str("name"),
            Self::DescriptionLength => f.write_str("description"),
            Self::ReasonLength => f.write_str("reason"),
            Self::CountingWindow => f.write_str("counting_window_days"),
            Self::TooManyFields => f.write_str("fields"),
            Self::FieldKey { index } | Self::FieldKeyDuplicate { index } => {
                write!(f, "fields[{index}].key")
            }
            Self::FieldLabelLength { index } => write!(f, "fields[{index}].label"),
            Self::LevelCount => f.write_str("levels"),
            Self::FirstLevelNotOne => f.write_str("levels[0].from_case_count"),
            Self::LevelCaseCount { index } | Self::LevelOrder { index } => {
                write!(f, "levels[{index}].from_case_count")
            }
            Self::TimeoutDuration { index } => write!(f, "levels[{index}].action.duration_seconds"),
            Self::BanDeleteWindow { index } => {
                write!(f, "levels[{index}].action.delete_message_seconds")
            }
        }
    }
}

impl RuleDefinition {
    /// Checks the definition at `strictness` and reports every problem at
    /// once, in document order. Text is measured after trimming.
    ///
    /// Name uniqueness, the active-rule cap, and the honeypot's no-required-
    /// fields rule depend on other rows; the engine checks those. Restoring
    /// an archived rule re-checks only those, never the definition.
    pub fn validate(&self, strictness: Strictness) -> Result<(), Vec<RuleProblem>> {
        let mut problems = Vec::new();
        match strictness {
            Strictness::New => self.validate_new(&mut problems),
            Strictness::Imported { reconstructed } => {
                self.validate_imported(reconstructed, &mut problems);
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    fn validate_new(&self, problems: &mut Vec<RuleProblem>) {
        if !limits::RULE_NAME.allows(&self.name) {
            problems.push(RuleProblem::NameLength);
        }
        if !limits::RULE_DESCRIPTION.allows(&self.description) {
            problems.push(RuleProblem::DescriptionLength);
        }
        if !limits::RULE_REASON.allows(&self.reason) {
            problems.push(RuleProblem::ReasonLength);
        }
        if let Some(days) = self.counting_window_days
            && !(limits::MIN_COUNTING_WINDOW_DAYS..=limits::MAX_COUNTING_WINDOW_DAYS)
                .contains(&days)
        {
            problems.push(RuleProblem::CountingWindow);
        }
        if self.fields.len() > limits::MAX_FIELDS {
            problems.push(RuleProblem::TooManyFields);
        }
        let mut seen = HashSet::new();
        for (index, field) in self.fields.iter().enumerate() {
            // Syntax and uniqueness are separate problems; both are reported.
            if !field.key.is_valid() {
                problems.push(RuleProblem::FieldKey { index });
            }
            if !seen.insert(field.key.as_str()) {
                problems.push(RuleProblem::FieldKeyDuplicate { index });
            }
            if !limits::FIELD_LABEL.allows(&field.label) {
                problems.push(RuleProblem::FieldLabelLength { index });
            }
        }
        if !(limits::MIN_LEVELS..=limits::MAX_LEVELS).contains(&self.levels.len()) {
            problems.push(RuleProblem::LevelCount);
        }
        let mut previous: Option<u32> = None;
        for (index, level) in self.levels.iter().enumerate() {
            let count = level.from_case_count;
            if index == 0 {
                if count != 1 {
                    problems.push(RuleProblem::FirstLevelNotOne);
                }
            } else {
                // Range and order are separate problems; both are reported.
                if !(2..=limits::MAX_LEVEL_CASE_COUNT).contains(&count) {
                    problems.push(RuleProblem::LevelCaseCount { index });
                }
                if previous.is_some_and(|p| count <= p) {
                    problems.push(RuleProblem::LevelOrder { index });
                }
            }
            previous = Some(count);
            check_action(index, level.action, true, problems);
        }
    }

    fn validate_imported(&self, reconstructed: bool, problems: &mut Vec<RuleProblem>) {
        if self.name.trim().is_empty() {
            problems.push(RuleProblem::NameLength);
        }
        if self.reason.trim().is_empty() {
            problems.push(RuleProblem::ReasonLength);
        }
        let mut seen = HashSet::new();
        for (index, field) in self.fields.iter().enumerate() {
            if !seen.insert(field.key.as_str()) {
                problems.push(RuleProblem::FieldKeyDuplicate { index });
            }
        }
        if self.levels.is_empty() || self.levels.len() > limits::MAX_IMPORTED_LEVELS {
            problems.push(RuleProblem::LevelCount);
        }
        let mut previous: Option<u32> = None;
        for (index, level) in self.levels.iter().enumerate() {
            let count = level.from_case_count;
            match previous {
                None if reconstructed && count == 0 => {
                    problems.push(RuleProblem::LevelCaseCount { index });
                }
                None if !reconstructed && count != 1 => {
                    problems.push(RuleProblem::FirstLevelNotOne);
                }
                Some(p) if count <= p => problems.push(RuleProblem::LevelOrder { index }),
                _ => {}
            }
            previous = Some(count);
            check_action(index, level.action, false, problems);
        }
    }
}

/// Checks a level's action: a timeout within 1–2,419,200 seconds always;
/// the ban deletion window only when `size_limits` apply.
fn check_action(
    index: usize,
    action: Option<LevelAction>,
    size_limits: bool,
    problems: &mut Vec<RuleProblem>,
) {
    match action {
        Some(LevelAction::Timeout { duration_seconds })
            if !(limits::MIN_TIMEOUT_SECONDS..=limits::MAX_TIMEOUT_SECONDS)
                .contains(&duration_seconds) =>
        {
            problems.push(RuleProblem::TimeoutDuration { index });
        }
        Some(LevelAction::Ban {
            delete_message_seconds,
        }) if size_limits && delete_message_seconds > limits::MAX_BAN_DELETE_SECONDS => {
            problems.push(RuleProblem::BanDeleteWindow { index });
        }
        _ => {}
    }
}
