//! Pure parts of the case mapping (§19.6): case context, v4 provenance,
//! and replacement-link validation.

use std::collections::{BTreeMap, HashMap, HashSet};

use quack_core::case::Decimal;
use serde_json::{Map, Value};
use uuid::Uuid;

/// What converting `context_values_json` noticed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContextNotes {
    /// The column was not an array of answers.
    pub not_an_array: bool,
    /// A key repeated; the first answer was kept.
    pub duplicate_key: bool,
    /// A number could not be written canonically and was kept as its text.
    pub number_kept: bool,
}

/// `context_values_json` (an array of `{key, value, …}`) → the `key →
/// value` object. Numbers become canonical decimal strings; empty and null
/// answers are absent.
pub(crate) fn context(raw: &Value) -> (Value, ContextNotes) {
    let mut notes = ContextNotes::default();
    let mut out = Map::new();
    let items = match raw {
        Value::Array(items) => items.as_slice(),
        Value::Null => &[],
        _ => {
            notes.not_an_array = true;
            &[]
        }
    };
    for item in items {
        let Some(key) = item.get("key").and_then(Value::as_str) else {
            notes.not_an_array = true;
            continue;
        };
        let value = match item.get("value") {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) if s.trim().is_empty() => continue,
            Some(Value::String(s)) => Value::String(s.clone()),
            Some(Value::Bool(b)) => Value::Bool(*b),
            Some(Value::Number(n)) => {
                let text = n.to_string();
                if let Ok(d) = Decimal::parse(&text) {
                    Value::String(d.to_string())
                } else {
                    notes.number_kept = true;
                    Value::String(text)
                }
            }
            Some(other) => Value::String(other.to_string()),
        };
        if out.contains_key(key) {
            notes.duplicate_key = true;
            continue;
        }
        out.insert(key.to_owned(), value);
    }
    (Value::Object(out), notes)
}

/// `cases.imported` for a v4 case: `{v4_action, v4_moderator, v4_reason,
/// v4_source_id}` from its metadata and reason.
pub(crate) fn v4_imported(metadata: &Value, reason: &str) -> Value {
    let v4 = metadata.get("v4").cloned().unwrap_or(Value::Null);
    let text = |key: &str| {
        v4.get(key)
            .cloned()
            .filter(|v| !v.is_null())
            .unwrap_or(Value::Null)
    };
    serde_json::json!({
        "v4_action": text("action_type"),
        "v4_moderator": text("moderator_display_name"),
        "v4_reason": reason,
        "v4_source_id": text("source_id"),
    })
}

/// One case's replacement claim: it replaces `target`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Claim {
    pub case: Uuid,
    pub target: Uuid,
}

/// Why a replacement link was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dropped {
    /// The replaced case is not an imported case of this server.
    NotInServer,
    /// A case cannot replace itself.
    Itself,
    /// The replaced case is not voided.
    NotVoided,
    /// Another case already replaces it.
    ReplacedTwice,
    /// The link would close a cycle.
    Cycle,
}

/// Keeps the valid replacement links (§19.6): same server, the replaced
/// case voided, not itself, not replaced twice (the first claim in input
/// order wins), and no cycles.
pub(crate) fn replacements(
    claims: &[Claim],
    voided: &HashMap<Uuid, bool>,
) -> (Vec<Claim>, Vec<(Claim, Dropped)>) {
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let mut replaced: HashSet<Uuid> = HashSet::new();
    let mut next: BTreeMap<Uuid, Uuid> = BTreeMap::new();
    for claim in claims {
        let reason = match voided.get(&claim.target) {
            _ if claim.case == claim.target => Some(Dropped::Itself),
            None => Some(Dropped::NotInServer),
            Some(false) => Some(Dropped::NotVoided),
            Some(true) if replaced.contains(&claim.target) => Some(Dropped::ReplacedTwice),
            Some(true) if reaches(&next, claim.target, claim.case) => Some(Dropped::Cycle),
            Some(true) => None,
        };
        if let Some(r) = reason {
            dropped.push((*claim, r));
        } else {
            replaced.insert(claim.target);
            next.insert(claim.case, claim.target);
            kept.push(*claim);
        }
    }
    (kept, dropped)
}

/// Whether following `replaces` links from `from` reaches `to`.
fn reaches(next: &BTreeMap<Uuid, Uuid>, from: Uuid, to: Uuid) -> bool {
    let mut at = from;
    let mut steps = 0;
    while let Some(n) = next.get(&at) {
        if *n == to {
            return true;
        }
        at = *n;
        steps += 1;
        if steps > next.len() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    #[test]
    fn context_values_become_an_object() {
        let (v, notes) = context(&json!([
            {"key": "a", "value": "text", "type": "short_text"},
            {"key": "b", "value": 2.50, "type": "number"},
            {"key": "c", "value": false, "type": "boolean"},
            {"key": "d", "value": "", "type": "short_text"},
            {"key": "a", "value": "again"},
            {"key": "e", "value": 1e300}
        ]));
        assert_eq!(
            v,
            json!({"a": "text", "b": "2.5", "c": false, "e": "1e+300"})
        );
        assert!(notes.duplicate_key && notes.number_kept && !notes.not_an_array);
        assert!(context(&json!({})).1.not_an_array);
        assert_eq!(context(&json!([])).0, json!({}));
    }

    #[test]
    fn v4_provenance() {
        let v = v4_imported(
            &json!({"historical": true, "v4": {"source_id": "9", "action_type": "ban", "moderator_display_name": "mod"}}),
            "r",
        );
        assert_eq!(
            v,
            json!({"v4_action": "ban", "v4_moderator": "mod", "v4_reason": "r", "v4_source_id": "9"})
        );
        assert_eq!(v4_imported(&json!({}), "")["v4_action"], Value::Null);
    }

    fn id(n: u8) -> Uuid {
        Uuid::from_u128(u128::from(n))
    }

    #[test]
    fn replacement_rules() {
        let voided: HashMap<Uuid, bool> =
            [(id(1), true), (id(2), false), (id(3), true), (id(4), true)].into();
        let claims = [
            Claim {
                case: id(5),
                target: id(1),
            },
            Claim {
                case: id(6),
                target: id(1),
            },
            Claim {
                case: id(7),
                target: id(2),
            },
            Claim {
                case: id(8),
                target: id(9),
            },
            Claim {
                case: id(3),
                target: id(3),
            },
            Claim {
                case: id(3),
                target: id(4),
            },
            Claim {
                case: id(4),
                target: id(3),
            },
        ];
        let (kept, dropped) = replacements(&claims, &voided);
        assert_eq!(kept, [claims[0], claims[5]]);
        let reasons: Vec<Dropped> = dropped.iter().map(|(_, r)| *r).collect();
        assert_eq!(
            reasons,
            [
                Dropped::ReplacedTwice,
                Dropped::NotVoided,
                Dropped::NotInServer,
                Dropped::Itself,
                Dropped::Cycle
            ]
        );
    }

    proptest! {
        #[test]
        fn kept_links_are_acyclic_and_unique(edges in proptest::collection::btree_map(0u8..8, 0u8..8, 0..8)) {
            let voided: HashMap<Uuid, bool> = (0..8).map(|n| (id(n), n % 3 != 0)).collect();
            let claims: Vec<Claim> = edges.iter().map(|(a, b)| Claim { case: id(*a), target: id(*b) }).collect();
            let (kept, _) = replacements(&claims, &voided);
            let mut targets = HashSet::new();
            let mut next = BTreeMap::new();
            for c in &kept {
                prop_assert!(targets.insert(c.target));
                prop_assert!(voided[&c.target]);
                prop_assert_ne!(c.case, c.target);
                next.insert(c.case, c.target);
            }
            for c in &kept {
                prop_assert!(!reaches(&next, c.target, c.case));
            }
        }
    }
}
