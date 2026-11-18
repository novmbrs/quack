//! Case context: answers to a rule's fields, and their validation (§4.6).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::link::{LinkProblem, MessageLink};
use crate::ids::GuildId;
use crate::limits;
use crate::rule::{Field, FieldKey, FieldKind};

/// One submitted answer, before validation.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextInput {
    /// From the API: any JSON value.
    Json(serde_json::Value),
    /// From a Discord modal or select: text (booleans are `yes`/`no`).
    Text(String),
}

/// One stored answer: a JSON string, bool, or number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContextValue {
    /// Text, or a canonical message link.
    Text(String),
    /// Yes or no.
    Boolean(bool),
    /// A number.
    Number(serde_json::Number),
}

/// A case's validated context: field key to answer. Absent keys are
/// unanswered optional fields. Stored as the `cases.context` JSON object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextValues(pub BTreeMap<FieldKey, ContextValue>);

impl ContextValues {
    /// The answer for `key`.
    pub fn get(&self, key: &FieldKey) -> Option<&ContextValue> {
        self.0.get(key)
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

/// Validates submitted answers against a rule version's fields and the
/// case's server, reporting every problem at once.
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
        ContextInput::Json(_) => false,
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
        ContextInput::Json(_) => None,
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
            let (value, decimals) = match input {
                ContextInput::Json(serde_json::Value::Number(n)) => json_number(n),
                ContextInput::Text(s) => parse_decimal(s.trim()).ok_or(P::NotANumber)?,
                ContextInput::Json(_) => return Err(P::NotANumber),
            };
            if !value.is_finite()
                || value.abs() > limits::CONTEXT_NUMBER_MAX_ABS
                || decimals > limits::CONTEXT_NUMBER_MAX_DECIMALS
            {
                return Err(P::NumberOutOfRange);
            }
            Ok(ContextValue::Number(store_number(value)))
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

/// A JSON number's value and its count of decimal places. Rust prints
/// floats in plain decimal (never exponent form), shortest round-trip.
#[allow(
    clippy::cast_precision_loss,
    reason = "the range check happens on the f64"
)]
fn json_number(n: &serde_json::Number) -> (f64, usize) {
    if let Some(i) = n.as_i64() {
        return (i as f64, 0);
    }
    if let Some(u) = n.as_u64() {
        return (u as f64, 0);
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    let decimals = f
        .to_string()
        .split_once('.')
        .map_or(0, |(_, frac)| frac.len());
    (f, decimals)
}

/// Parses Discord text as a decimal: optional `-`, digits, optional `.`
/// and digits. No exponent, no `+`, no bare `.5` or `5.`.
fn parse_decimal(s: &str) -> Option<(f64, usize)> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match digits.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (digits, None),
    };
    let all_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int) || frac.is_some_and(|f| !all_digits(f)) {
        return None;
    }
    Some((s.parse().ok()?, frac.map_or(0, str::len)))
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "whole values are at most 10^15"
)]
fn store_number(value: f64) -> serde_json::Number {
    if value.fract() == 0.0 {
        serde_json::Number::from(value as i64)
    } else {
        serde_json::Number::from_f64(value).unwrap_or_else(|| serde_json::Number::from(0))
    }
}

#[cfg(test)]
mod tests;
