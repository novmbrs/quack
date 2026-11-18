//! Rules: the definition document every rule version stores (§3.2), its
//! validation (§3.3), versioning, the import/export file (§3.6), and the two
//! rules Quack creates itself (§3.7, §3.8).

mod file;
mod validate;

use serde::{Deserialize, Serialize};

use crate::DomainError;
use crate::action::ActionKind;

pub use file::{RULE_FILE_FORMAT, RULE_FILE_FORMAT_VERSION, RuleFile, RuleFileError};
pub use validate::{RuleProblem, RuleProblemCode, Strictness};

/// One rule version's full definition: the `rule_versions.definition`
/// column, the API body, and the export file's `rule` all use this JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDefinition {
    /// Shown to staff and members; unique among the server's active rules.
    pub name: String,
    /// Staff-facing explanation; may be empty.
    pub description: String,
    /// The official reason shown to the member. No placeholders.
    pub reason: String,
    /// Whether cases under this version can be appealed.
    pub appealable: bool,
    /// `None` counts every past case; otherwise only cases created within
    /// the last N days count toward escalation.
    pub counting_window_days: Option<u16>,
    /// Case context fields, in display order.
    pub fields: Vec<Field>,
    /// Escalation levels, ascending by `from_case_count`.
    pub levels: Vec<Level>,
}

/// One case context field: a question the moderator answers and the member
/// sees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Stable machine key, unique within the rule.
    pub key: FieldKey,
    /// The question shown to the moderator and the label shown to the member.
    pub label: String,
    /// What kind of answer it takes.
    pub kind: FieldKind,
    /// Whether it must be answered when the rule is applied.
    pub required: bool,
}

/// A field's machine key. Valid keys match `^[a-z0-9][a-z0-9_-]{1,63}$`;
/// [`RuleDefinition::validate`] reports invalid ones rather than parsing
/// failing, so every problem is listed at once.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldKey(pub String);

impl FieldKey {
    /// Whether the key matches `^[a-z0-9][a-z0-9_-]{1,63}$`.
    pub fn is_valid(&self) -> bool {
        let bytes = self.0.as_bytes();
        let lower_or_digit = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        (2..=64).contains(&bytes.len())
            && bytes.first().is_some_and(lower_or_digit)
            && bytes[1..]
                .iter()
                .all(|b| lower_or_digit(b) || *b == b'_' || *b == b'-')
    }

    /// The key as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

str_enum! {
    /// What kind of answer a case context field takes (§4.6).
    pub enum FieldKind {
        /// One line of text, 1–500 characters.
        ShortText = "short_text",
        /// A paragraph, 1–4,000 characters.
        LongText = "long_text",
        /// Yes or no.
        Boolean = "boolean",
        /// A decimal number.
        Number = "number",
        /// A link to a message in this server.
        MessageLink = "message_link",
    }
}

/// One rung of a rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    /// The case count at which this level starts. The first level starts at 1.
    pub from_case_count: u16,
    /// The one action the level takes; `None` makes the case a warning.
    pub action: Option<LevelAction>,
    /// Whether the member gets a case DM.
    pub send_dm: bool,
}

/// The action a level takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "RawLevelAction")]
pub enum LevelAction {
    /// Time the member out until `created_at + duration_seconds`.
    Timeout {
        /// How long, in seconds (1–2,419,200).
        duration_seconds: u32,
    },
    /// Remove the member from the server.
    Kick,
    /// Ban the member.
    Ban {
        /// How much of the member's recent messages Discord deletes, in
        /// seconds (0–604,800).
        delete_message_seconds: u32,
    },
}

/// The JSON shape of a level action, read strictly: serde's tagged enums
/// can't reject unknown fields on `{"kind": "kick"}`, so this does.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLevelAction {
    kind: String,
    duration_seconds: Option<u32>,
    delete_message_seconds: Option<u32>,
}

impl TryFrom<RawLevelAction> for LevelAction {
    type Error = String;

    fn try_from(raw: RawLevelAction) -> Result<Self, String> {
        match (
            raw.kind.as_str(),
            raw.duration_seconds,
            raw.delete_message_seconds,
        ) {
            ("timeout", Some(duration_seconds), None) => Ok(Self::Timeout { duration_seconds }),
            ("kick", None, None) => Ok(Self::Kick),
            ("ban", None, Some(delete_message_seconds)) => Ok(Self::Ban {
                delete_message_seconds,
            }),
            (kind, ..) => Err(format!("invalid level action of kind {kind:?}")),
        }
    }
}

impl LevelAction {
    /// The action kind this level records on its cases.
    pub const fn kind(self) -> ActionKind {
        match self {
            Self::Timeout { .. } => ActionKind::Timeout,
            Self::Kick => ActionKind::Kick,
            Self::Ban { .. } => ActionKind::Ban,
        }
    }
}

impl RuleDefinition {
    /// Returns the definition with every text value trimmed, as it is stored.
    /// Field keys are left alone: whitespace in a key is a problem to report.
    #[must_use]
    pub fn trimmed(mut self) -> Self {
        fn trim(s: &mut String) {
            let t = s.trim();
            if t.len() != s.len() {
                *s = t.to_owned();
            }
        }
        trim(&mut self.name);
        trim(&mut self.description);
        trim(&mut self.reason);
        for field in &mut self.fields {
            trim(&mut field.label);
        }
        self
    }

    /// Whether any field is required (the honeypot can't fill those in).
    pub fn has_required_fields(&self) -> bool {
        self.fields.iter().any(|f| f.required)
    }

    /// Refuses a definition the honeypot can't apply while the honeypot
    /// uses this rule (§3.3, §3.8).
    pub fn check_honeypot_compatible(&self) -> Result<(), DomainError> {
        if self.has_required_fields() {
            Err(DomainError::RequiredFieldOnHoneypotRule)
        } else {
            Ok(())
        }
    }

    /// Looks up a field by key.
    pub fn field(&self, key: &FieldKey) -> Option<&Field> {
        self.fields.iter().find(|f| &f.key == key)
    }
}

/// Returns the version number an edit writes, refusing a stale editor:
/// the client's `expected` must equal the rule's `current` version (§3.1).
pub fn next_version(current: u32, expected: u32) -> Result<u32, DomainError> {
    if current == expected {
        current.checked_add(1).ok_or(DomainError::RuleChanged)
    } else {
        Err(DomainError::RuleChanged)
    }
}

/// The rule Quack creates the first time it joins a server (§3.7).
pub fn starter_rule() -> RuleDefinition {
    RuleDefinition {
        name: "General rule violation".to_owned(),
        description: "A starter rule. Review it and change it to fit your server.".to_owned(),
        reason: "Breaking the server rules.".to_owned(),
        appealable: true,
        counting_window_days: None,
        fields: Vec::new(),
        levels: vec![
            Level {
                from_case_count: 1,
                action: None,
                send_dm: true,
            },
            Level {
                from_case_count: 3,
                action: Some(LevelAction::Timeout {
                    duration_seconds: 86_400,
                }),
                send_dm: true,
            },
            Level {
                from_case_count: 5,
                action: Some(LevelAction::Ban {
                    delete_message_seconds: 86_400,
                }),
                send_dm: true,
            },
        ],
    }
}

/// The name of the rule `/setup` uses or creates for the honeypot (§3.8).
pub const HONEYPOT_RULE_NAME: &str = "Honeypot";

/// The rule `/setup` creates for the honeypot when none is chosen and no
/// active rule is named "Honeypot" (§3.8).
pub fn honeypot_rule() -> RuleDefinition {
    RuleDefinition {
        name: HONEYPOT_RULE_NAME.to_owned(),
        description: "Applied when a member posts in the honeypot channel.".to_owned(),
        reason: "Posting in the honeypot channel.".to_owned(),
        appealable: true,
        counting_window_days: None,
        fields: Vec::new(),
        levels: vec![Level {
            from_case_count: 1,
            action: Some(LevelAction::Ban {
                delete_message_seconds: 0,
            }),
            send_dm: true,
        }],
    }
}

#[cfg(test)]
mod tests;
