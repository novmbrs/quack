//! A template's current version from its child rows (§19.5 step 2).

use std::collections::BTreeSet;

use quack_core::rule::{
    Field, FieldKey, FieldKind, Level, LevelAction, RuleDefinition, RuleProblemCode, Strictness,
};
use serde_json::Value;

/// A `case_templates` row's own fields.
#[derive(Clone, Debug, Default)]
pub(crate) struct Template {
    pub name: String,
    pub description: String,
    pub reason: String,
    pub appealable: bool,
    pub decay_days: i64,
}

/// A `case_template_context_fields` row.
#[derive(Clone, Debug)]
pub(crate) struct FieldRow {
    pub key: String,
    pub label: String,
    pub field_type: String,
    pub position: i64,
    pub required: bool,
}

/// A `case_template_levels` row with its action.
#[derive(Clone, Debug)]
pub(crate) struct LevelRow {
    pub position: i64,
    pub is_default: bool,
    pub trigger: i64,
    pub notify: bool,
    /// `(action_type, config_json)` from `case_template_level_actions`.
    pub action: Option<(String, Value)>,
}

/// What building the current version noticed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notes {
    /// A level at case count 1 replaced the default level.
    pub dropped_default: bool,
    /// Size limits the definition exceeds (kept, reported).
    pub over_limit: Vec<RuleProblemCode>,
}

/// Go field types to rule field kinds.
pub(crate) fn field_kind(raw: &str) -> Option<FieldKind> {
    match raw {
        "discord_message_link" => Some(FieldKind::MessageLink),
        other => other.parse().ok(),
    }
}

/// `case_decay_days`: 0 → no window.
pub(crate) fn window(days: i64) -> Result<Option<u16>, String> {
    if days == 0 {
        return Ok(None);
    }
    u16::try_from(days)
        .ok()
        .filter(|d| *d > 0)
        .map(Some)
        .ok_or_else(|| format!("case_decay_days {days} is out of range"))
}

/// A level action from its Go type and config (§19.5): timeout needs a
/// duration; a ban without a window deletes nothing; `send_dm` (and no
/// type) is a warning.
pub(crate) fn level_action(
    action_type: &str,
    config: &Value,
) -> Result<Option<LevelAction>, String> {
    let seconds = |key: &str| -> Result<Option<u32>, String> {
        match config.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Some)
                .ok_or_else(|| format!("{key} {v} is not a valid number of seconds")),
        }
    };
    match action_type {
        "timeout_user" => {
            let duration_seconds =
                seconds("duration_seconds")?.ok_or("a timeout level has no duration_seconds")?;
            Ok(Some(LevelAction::Timeout { duration_seconds }))
        }
        "kick_user" => Ok(Some(LevelAction::Kick)),
        "ban_user" => Ok(Some(LevelAction::Ban {
            delete_message_seconds: seconds("delete_message_seconds")?.unwrap_or(0),
        })),
        "" | "send_dm" => Ok(None),
        other => Err(format!("unknown level action {other:?}")),
    }
}

/// Builds and validates the current version. A structural failure lists
/// every problem; the server's import then aborts.
pub(crate) fn build(
    t: &Template,
    fields: &[FieldRow],
    levels: &[LevelRow],
) -> Result<(RuleDefinition, Notes), Vec<String>> {
    let mut problems = Vec::new();
    let mut notes = Notes::default();
    let mut sorted_fields = fields.to_vec();
    sorted_fields.sort_by_key(|f| f.position);
    let fields: Vec<Field> = sorted_fields
        .iter()
        .filter_map(|f| {
            let kind = field_kind(&f.field_type);
            if kind.is_none() {
                problems.push(format!(
                    "field {:?} has unknown type {:?}",
                    f.key, f.field_type
                ));
            }
            Some(Field {
                key: FieldKey(f.key.clone()),
                label: f.label.clone(),
                kind: kind?,
                required: f.required,
            })
        })
        .collect();
    let has_one = levels.iter().any(|l| !l.is_default && l.trigger == 1);
    let mut out = Vec::new();
    for l in levels {
        if l.is_default && has_one {
            notes.dropped_default = true;
            continue;
        }
        let from = if l.is_default { 1 } else { l.trigger };
        let Ok(from) = u32::try_from(from) else {
            problems.push(format!(
                "level at position {} has trigger_case_count {}",
                l.position, l.trigger
            ));
            continue;
        };
        let action = match &l.action {
            None => None,
            Some((kind, config)) => match level_action(kind, config) {
                Ok(a) => a,
                Err(e) => {
                    problems.push(format!("level at position {}: {e}", l.position));
                    None
                }
            },
        };
        out.push((
            from,
            l.position,
            Level {
                from_case_count: from,
                action,
                send_dm: l.notify,
            },
        ));
    }
    out.sort_by_key(|(from, position, _)| (*from, *position));
    let counting_window_days = window(t.decay_days).unwrap_or_else(|e| {
        problems.push(e);
        None
    });
    let def = RuleDefinition {
        name: t.name.clone(),
        description: t.description.clone(),
        reason: t.reason.clone(),
        appealable: t.appealable,
        counting_window_days,
        fields,
        levels: out.into_iter().map(|(.., l)| l).collect(),
    }
    .trimmed();
    if let Err(found) = def.validate(Strictness::Imported {
        reconstructed: false,
    }) {
        problems.extend(
            found
                .iter()
                .map(|p| format!("{} ({})", p.path(), p.code().as_str())),
        );
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    notes.over_limit = over_limit(&def);
    Ok((def, notes))
}

/// The size limits a valid imported definition exceeds.
pub(crate) fn over_limit(def: &RuleDefinition) -> Vec<RuleProblemCode> {
    let codes: BTreeSet<RuleProblemCode> = match def.validate(Strictness::New) {
        Ok(()) => BTreeSet::new(),
        Err(problems) => problems.iter().map(|p| p.code()).collect(),
    };
    codes.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t() -> Template {
        Template {
            name: " Spam ".into(),
            description: String::new(),
            reason: "No spam".into(),
            appealable: true,
            decay_days: 30,
        }
    }

    fn level(
        position: i64,
        is_default: bool,
        trigger: i64,
        action: Option<(&str, Value)>,
    ) -> LevelRow {
        LevelRow {
            position,
            is_default,
            trigger,
            notify: true,
            action: action.map(|(k, v)| (k.to_owned(), v)),
        }
    }

    #[test]
    fn default_level_starts_at_one() {
        let levels = [
            level(1, true, 0, None),
            level(
                2,
                false,
                3,
                Some(("timeout_user", json!({"duration_seconds": 600}))),
            ),
            level(3, false, 5, Some(("ban_user", json!({})))),
        ];
        let (def, notes) = build(&t(), &[], &levels).unwrap();
        assert_eq!(def.name, "Spam");
        assert_eq!(def.counting_window_days, Some(30));
        let from: Vec<u32> = def.levels.iter().map(|l| l.from_case_count).collect();
        assert_eq!(from, [1, 3, 5]);
        assert_eq!(
            def.levels[2].action,
            Some(LevelAction::Ban {
                delete_message_seconds: 0
            })
        );
        assert_eq!(notes, Notes::default());
    }

    #[test]
    fn a_level_at_one_replaces_the_default() {
        let levels = [
            level(1, true, 0, None),
            level(2, false, 1, Some(("kick_user", json!({})))),
        ];
        let (def, notes) = build(&t(), &[], &levels).unwrap();
        assert_eq!(def.levels.len(), 1);
        assert_eq!(def.levels[0].action, Some(LevelAction::Kick));
        assert!(notes.dropped_default);
    }

    #[test]
    fn structural_problems_are_all_listed() {
        let levels = [level(1, false, 2, Some(("explode", json!({}))))];
        let fields = [FieldRow {
            key: "x".into(),
            label: "X".into(),
            field_type: "colour".into(),
            position: 1,
            required: false,
        }];
        let problems = build(&t(), &fields, &levels).unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    #[test]
    fn over_limits_are_reported_not_refused() {
        let mut long = t();
        long.name = "n".repeat(150);
        let (_, notes) = build(&long, &[], &[level(1, true, 0, None)]).unwrap();
        assert_eq!(notes.over_limit, [RuleProblemCode::NameLength]);
    }

    #[test]
    fn level_actions() {
        assert_eq!(level_action("send_dm", &json!({})), Ok(None));
        assert!(level_action("timeout_user", &json!({})).is_err());
        assert!(level_action("timeout_user", &json!({"duration_seconds": -1})).is_err());
        assert_eq!(
            level_action("ban_user", &json!({"delete_message_seconds": 60})),
            Ok(Some(LevelAction::Ban {
                delete_message_seconds: 60
            }))
        );
    }
}
