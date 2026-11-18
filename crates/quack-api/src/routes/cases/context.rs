//! A submitted case context object, read as raw `(key, JSON text)` pairs:
//! numbers keep their exact digits (`ContextInput::from_json_text`), and a
//! key sent twice reaches the engine as two answers (`duplicate_field`)
//! instead of the last one silently winning.

use std::fmt;

use quack_core::case::ContextInput;
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

use crate::error::{ApiError, ApiResult};

/// A context object in a request body.
#[derive(Debug, Default)]
pub(crate) struct ContextBody(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for ContextBody {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Pairs;

        impl<'de> Visitor<'de> for Pairs {
            type Value = ContextBody;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of field answers")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ContextBody, A::Error> {
                let mut pairs = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    pairs.push((key, value));
                }
                Ok(ContextBody(pairs))
            }
        }

        d.deserialize_map(Pairs)
    }
}

impl ContextBody {
    /// The answers as the engine takes them.
    pub(crate) fn into_inputs(self) -> ApiResult<Vec<(String, ContextInput)>> {
        self.0
            .into_iter()
            .map(|(key, raw)| {
                ContextInput::from_json_text(raw.get())
                    .map(|input| (key, input))
                    .map_err(|_| ApiError::api(quack_text::api::ApiCode::InvalidJson))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_their_digits_and_duplicates_survive() {
        let body: ContextBody =
            serde_json::from_str(r#"{"amount": 2.50, "note": "hi", "amount": 1e400, "ok": true}"#)
                .unwrap_or_default();
        let inputs = body.into_inputs().unwrap_or_default();
        assert_eq!(
            inputs,
            vec![
                (
                    "amount".to_owned(),
                    ContextInput::JsonNumber("2.50".to_owned())
                ),
                (
                    "note".to_owned(),
                    ContextInput::Json(serde_json::json!("hi"))
                ),
                (
                    "amount".to_owned(),
                    ContextInput::JsonNumber("1e400".to_owned())
                ),
                ("ok".to_owned(), ContextInput::Json(serde_json::json!(true))),
            ]
        );
    }
}
