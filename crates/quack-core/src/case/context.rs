//! Case context: answers to a rule's fields, and their validation (§4.6).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::decimal::{Decimal, DecimalError};
use super::link::{LinkProblem, MessageLink, find_message_links};
use crate::ids::GuildId;
use crate::limits;
use crate::rule::{Field, FieldKey, FieldKind};

/// One submitted answer, before validation.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextInput {
    /// From the API: a JSON value. A JSON number here has already been
    /// read by `serde_json`: integers are exact, but any other number is
    /// read from its shortest float form, so the API passes numbers as
    /// [`ContextInput::JsonNumber`] instead (see
    /// [`ContextInput::from_json_text`]).
    Json(serde_json::Value),
    /// From the API: the original text of a JSON number, e.g. `2.50`.
    JsonNumber(String),
    /// From a Discord modal or select: text (booleans are `yes`/`no`).
    Text(String),
}

impl ContextInput {
    /// Reads one API answer from its JSON text (for example a
    /// `serde_json::value::RawValue`), keeping a number's original text so
    /// it is never parsed through a binary float.
    pub fn from_json_text(raw: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(raw)?;
        Ok(if value.is_number() {
            Self::JsonNumber(raw.trim().to_owned())
        } else {
            Self::Json(value)
        })
    }
}

/// One stored answer: text, yes/no, or an exact decimal. Numbers are
/// stored and sent as their canonical decimal string, so reading stored
/// JSON gives [`ContextValue::Text`] for them until
/// [`ContextValues::with_kinds`] types them by field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContextValue {
    /// Text, or a canonical message link.
    Text(String),
    /// Yes or no.
    Boolean(bool),
    /// A number.
    Number(Decimal),
}

impl ContextValue {
    /// The answer as a number: a [`ContextValue::Number`], or text that is
    /// a canonical decimal (as stored).
    pub fn as_decimal(&self) -> Option<Decimal> {
        match self {
            Self::Number(d) => Some(*d),
            Self::Text(s) => Decimal::parse(s).ok(),
            Self::Boolean(_) => None,
        }
    }
}

/// A case's validated context: field key to answer. Absent keys are
/// unanswered optional fields. Stored as the `cases.context` JSON object.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextValues(pub BTreeMap<FieldKey, ContextValue>);

impl ContextValues {
    /// The answer for `key`.
    pub fn get(&self, key: &FieldKey) -> Option<&ContextValue> {
        self.0.get(key)
    }

    /// Types stored answers by their fields: text in a `number` field that
    /// is a canonical decimal becomes [`ContextValue::Number`]. Use it after
    /// reading `cases.context`.
    #[must_use]
    pub fn with_kinds(mut self, fields: &[Field]) -> Self {
        for field in fields.iter().filter(|f| f.kind == FieldKind::Number) {
            if let Some(value) = self.0.get_mut(&field.key)
                && let Some(d) = value.as_decimal()
            {
                *value = ContextValue::Number(d);
            }
        }
        self
    }

    /// The evidence sources the answers name (§4.2 step 4, §4.9): every
    /// `message_link` answer, then every message link inside a text answer
    /// (§4.6), in field order, without repeated messages. Links inside text
    /// that point outside `guild` are ignored.
    pub fn evidence_links(&self, fields: &[Field], guild: GuildId) -> Vec<MessageLink> {
        let mut links = self.message_links(fields);
        for field in fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::ShortText | FieldKind::LongText))
        {
            if let Some(ContextValue::Text(text)) = self.0.get(&field.key) {
                for link in find_message_links(text) {
                    if link.guild_id == guild
                        && !links.iter().any(|l| l.message_id == link.message_id)
                    {
                        links.push(link);
                    }
                }
            }
        }
        links
    }

    /// Every message link answered in a `message_link` field, in field order.
    pub fn message_links(&self, fields: &[Field]) -> Vec<MessageLink> {
        fields
            .iter()
            .filter(|f| f.kind == FieldKind::MessageLink)
            .filter_map(|f| match self.0.get(&f.key) {
                Some(ContextValue::Text(url)) => MessageLink::parse(url).ok(),
                _ => None,
            })
            .collect()
    }

    /// Keys whose answer differs between `self` and `new` (added, removed,
    /// or changed), sorted; for the `case_context_edited` log entry.
    pub fn changed_keys(&self, new: &Self) -> Vec<FieldKey> {
        let keys: std::collections::BTreeSet<&FieldKey> =
            self.0.keys().chain(new.0.keys()).collect();
        keys.into_iter()
            .filter(|k| self.0.get(*k) != new.0.get(*k))
            .cloned()
            .collect()
    }
}

str_enum! {
    /// The machine code of one context problem.
    pub enum ContextProblemCode {
        /// The key is not one of the rule's fields.
        UnknownField = "unknown_field",
        /// The key was submitted more than once.
        DuplicateField = "duplicate_field",
        /// A required field has no answer.
        Required = "required",
        /// Text is longer than the field allows.
        TooLong = "too_long",
        /// A text field got something other than text.
        NotText = "not_text",
        /// A boolean field got something other than yes or no.
        NotYesNo = "not_yes_no",
        /// A number field got something that is not a number.
        NotANumber = "not_a_number",
        /// A number is too large or has more than 6 decimal places.
        NumberOutOfRange = "number_out_of_range",
        /// A message link field got something that is not a message link.
        NotAMessageLink = "not_a_message_link",
        /// A message link points into another server.
        OtherServer = "other_server",
    }
}

/// One problem with submitted context: which key, and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextProblem {
    /// The submitted or field key.
    pub key: String,
    /// What is wrong.
    pub code: ContextProblemCode,
}

impl ContextProblem {
    /// The API field path, `context.<key>`.
    pub fn path(&self) -> String {
        format!("context.{}", self.key)
    }
}

/// Validates submitted answers against a rule version's context fields
/// ([`crate::rule::RuleDefinition::context_fields`]) and the case's server,
/// reporting every problem at once.
///
/// An omitted, null, empty, or whitespace-only optional answer is stored as
/// absent; `false` and `0` are answers. Text is stored trimmed.
pub fn validate_context(
    fields: &[Field],
    guild: GuildId,
    submitted: &[(String, ContextInput)],
) -> Result<ContextValues, Vec<ContextProblem>> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for (key, _) in submitted {
        let problem = |code| ContextProblem {
            key: key.clone(),
            code,
        };
        if !fields.iter().any(|f| f.key.as_str() == key) {
            problems.push(problem(ContextProblemCode::UnknownField));
        } else if !seen.insert(key.as_str()) {
            problems.push(problem(ContextProblemCode::DuplicateField));
        }
    }
    let mut values = BTreeMap::new();
    for field in fields {
        let input = submitted
            .iter()
            .find(|(k, _)| k == field.key.as_str())
            .map(|(_, v)| v);
        match input.filter(|v| !is_blank(v)) {
            None if field.required => problems.push(ContextProblem {
                key: field.key.as_str().to_owned(),
                code: ContextProblemCode::Required,
            }),
            None => {}
            Some(input) => match validate_one(field.kind, guild, input) {
                Ok(value) => {
                    values.insert(field.key.clone(), value);
                }
                Err(code) => problems.push(ContextProblem {
                    key: field.key.as_str().to_owned(),
                    code,
                }),
            },
        }
    }
    if problems.is_empty() {
        Ok(ContextValues(values))
    } else {
        Err(problems)
    }
}

/// Fills every unanswered `message_link` field with `link` (the message a
/// case was started from), before validation (§4.2 step 4).
pub fn fill_message_links(
    fields: &[Field],
    submitted: &mut Vec<(String, ContextInput)>,
    link: &MessageLink,
) {
    for field in fields.iter().filter(|f| f.kind == FieldKind::MessageLink) {
        let key = field.key.as_str();
        match submitted.iter_mut().find(|(k, _)| k == key) {
            Some((_, value)) if is_blank(value) => *value = ContextInput::Text(link.to_url()),
            Some(_) => {}
            None => submitted.push((key.to_owned(), ContextInput::Text(link.to_url()))),
        }
    }
}

fn is_blank(input: &ContextInput) -> bool {
    match input {
        ContextInput::Json(serde_json::Value::Null) => true,
        ContextInput::Json(serde_json::Value::String(s)) | ContextInput::Text(s) => {
            s.trim().is_empty()
        }
        ContextInput::Json(_) | ContextInput::JsonNumber(_) => false,
    }
}

fn validate_one(
    kind: FieldKind,
    guild: GuildId,
    input: &ContextInput,
) -> Result<ContextValue, ContextProblemCode> {
    use ContextProblemCode as P;
    let text = match input {
        ContextInput::Json(serde_json::Value::String(s)) | ContextInput::Text(s) => {
            Some(s.as_str())
        }
        ContextInput::Json(_) | ContextInput::JsonNumber(_) => None,
    };
    match kind {
        FieldKind::ShortText | FieldKind::LongText => {
            let text = text.ok_or(P::NotText)?;
            let limit = if kind == FieldKind::ShortText {
                limits::CONTEXT_SHORT_TEXT
            } else {
                limits::CONTEXT_LONG_TEXT
            };
            if limit.allows(text) {
                Ok(ContextValue::Text(text.trim().to_owned()))
            } else {
                Err(P::TooLong)
            }
        }
        FieldKind::Boolean => match input {
            ContextInput::Json(serde_json::Value::Bool(b)) => Ok(ContextValue::Boolean(*b)),
            ContextInput::Text(s) if s.trim() == "yes" => Ok(ContextValue::Boolean(true)),
            ContextInput::Text(s) if s.trim() == "no" => Ok(ContextValue::Boolean(false)),
            _ => Err(P::NotYesNo),
        },
        FieldKind::Number => {
            let text = match input {
                ContextInput::JsonNumber(raw) => raw.trim().to_owned(),
                ContextInput::Json(serde_json::Value::Number(n)) => n.to_string(),
                ContextInput::Json(serde_json::Value::String(s)) | ContextInput::Text(s) => {
                    s.trim().to_owned()
                }
                ContextInput::Json(_) => return Err(P::NotANumber),
            };
            Decimal::parse(&text)
                .map(ContextValue::Number)
                .map_err(|e| match e {
                    DecimalError::NotANumber => P::NotANumber,
                    DecimalError::OutOfRange => P::NumberOutOfRange,
                })
        }
        FieldKind::MessageLink => {
            let text = text.ok_or(P::NotAMessageLink)?;
            MessageLink::parse_in(text, guild)
                .map(|l| ContextValue::Text(l.to_url()))
                .map_err(|e| match e {
                    LinkProblem::NotAMessageLink => P::NotAMessageLink,
                    LinkProblem::OtherServer => P::OtherServer,
                })
        }
    }
}

#[cfg(test)]
mod tests;
