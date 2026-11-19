//! The portable rule file for export and import (§3.6).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{RuleDefinition, RuleProblem, Strictness};

/// The `format` value every rule file carries.
pub const RULE_FILE_FORMAT: &str = "quack-rule";
/// The only `format_version` Quack writes and reads.
pub const RULE_FILE_FORMAT_VERSION: u32 = 1;

/// An exported rule. It never contains server ids, channel ids, case data,
/// member history, or settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    /// Always [`RULE_FILE_FORMAT`].
    pub format: String,
    /// Always [`RULE_FILE_FORMAT_VERSION`].
    pub format_version: u32,
    /// When the file was exported.
    #[serde(with = "time::serde::rfc3339")]
    pub exported_at: OffsetDateTime,
    /// The rule's current definition.
    pub rule: RuleDefinition,
}

/// Why an imported rule file was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuleFileError {
    /// `format` is not `"quack-rule"`.
    #[error("not a Quack rule file")]
    WrongFormat,
    /// `format_version` is one this Quack does not read.
    #[error("unsupported rule file version {0}")]
    UnsupportedVersion(u32),
    /// The rule inside fails validation as a new rule.
    #[error("the rule has {} problem(s)", .0.len())]
    Invalid(Vec<RuleProblem>),
}

impl RuleFile {
    /// Wraps a definition for export at `exported_at`.
    pub fn new(rule: RuleDefinition, exported_at: OffsetDateTime) -> Self {
        Self {
            format: RULE_FILE_FORMAT.to_owned(),
            format_version: RULE_FILE_FORMAT_VERSION,
            exported_at,
            rule,
        }
    }

    /// Checks an uploaded file and returns its rule, trimmed and validated as
    /// a new rule (`Strictness::New`). Malformed JSON is the caller's
    /// `invalid_json`; this only judges a file that parsed.
    pub fn into_rule(self) -> Result<RuleDefinition, RuleFileError> {
        if self.format != RULE_FILE_FORMAT {
            return Err(RuleFileError::WrongFormat);
        }
        if self.format_version != RULE_FILE_FORMAT_VERSION {
            return Err(RuleFileError::UnsupportedVersion(self.format_version));
        }
        let rule = self.rule.trimmed();
        rule.validate(Strictness::New)
            .map_err(RuleFileError::Invalid)?;
        Ok(rule)
    }
}
