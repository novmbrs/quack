//! `cases.template_snapshot_json`: the decision a case recorded, and the
//! partial rule versions rebuilt from those snapshots (§19.5 step 3, §19.6).

use std::collections::BTreeMap;

use quack_core::rule::{Field, FieldKey, Level, LevelAction, RuleDefinition};
use serde::Deserialize;

use super::current::field_kind;

/// The Go snapshot. Every part is optional: old rows and the v4 variant
/// leave most of it out.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Snapshot {
    #[serde(default)]
    pub template: Option<SnapTemplate>,
    #[serde(default)]
    pub selected_level: Option<SnapLevel>,
    #[serde(default)]
    pub actions: Option<Vec<SnapAction>>,
    #[serde(default)]
    pub context_fields: Option<Vec<SnapField>>,
}

/// `template` in the snapshot.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SnapTemplate {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reason_template: Option<String>,
    #[serde(default)]
    pub case_decay_days: Option<i64>,
    #[serde(default)]
    pub appealable: Option<bool>,
}

/// `selected_level` in the snapshot.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SnapLevel {
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub trigger_case_count: Option<i64>,
    #[serde(default)]
    pub notify_user: Option<bool>,
    #[serde(default)]
    pub matched_case_count: Option<i64>,
}

/// One of `actions`.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SnapAction {
    #[serde(default)]
    pub action_type: String,
    #[serde(default)]
    pub timeout_duration_seconds: Option<i64>,
    #[serde(default)]
    pub delete_message_seconds: Option<i64>,
}

/// One of `context_fields`.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct SnapField {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub required: Option<bool>,
}

impl Snapshot {
    /// Parses the JSON; anything that is not this shape reads as empty.
    pub(crate) fn parse(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

/// What a case decided, from its snapshot (§19.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decision {
    /// The level, as a rule level.
    pub level: Level,
    /// Its Go position (1-based), when the snapshot has a usable one.
    pub position: Option<i16>,
    /// `matched_case_count`.
    pub case_count: i32,
}

/// The case's decision, or why there is none (the server then aborts).
pub(crate) fn decision(s: &Snapshot) -> Result<Decision, String> {
    let level = s
        .selected_level
        .as_ref()
        .ok_or("the snapshot has no selected_level")?;
    let action = snapshot_action(s.actions.as_deref().unwrap_or_default())?;
    let trigger = level.trigger_case_count.unwrap_or(0);
    let from = if level.is_default.unwrap_or(false) || trigger <= 0 {
        1
    } else {
        u32::try_from(trigger)
            .map_err(|_| format!("trigger_case_count {trigger} is out of range"))?
    };
    let count = level.matched_case_count.unwrap_or(0);
    let case_count = i32::try_from(count.max(1))
        .map_err(|_| format!("matched_case_count {count} is out of range"))?;
    Ok(Decision {
        level: Level {
            from_case_count: from,
            action,
            send_dm: level.notify_user.unwrap_or(false),
        },
        position: level
            .position
            .filter(|p| *p >= 1)
            .and_then(|p| i16::try_from(p).ok()),
        case_count,
    })
}

/// The first timeout, kick, or ban in the snapshot's actions.
fn snapshot_action(actions: &[SnapAction]) -> Result<Option<LevelAction>, String> {
    for a in actions {
        let action = match a.action_type.as_str() {
            "timeout_user" => {
                let secs = a
                    .timeout_duration_seconds
                    .ok_or("a timeout in the snapshot has no timeout_duration_seconds")?;
                LevelAction::Timeout {
                    duration_seconds: u32::try_from(secs)
                        .map_err(|_| format!("timeout of {secs}s is out of range"))?,
                }
            }
            "kick_user" => LevelAction::Kick,
            "ban_user" => {
                let secs = a.delete_message_seconds.unwrap_or(0);
                LevelAction::Ban {
                    delete_message_seconds: u32::try_from(secs)
                        .map_err(|_| format!("ban deletion window of {secs}s is out of range"))?,
                }
            }
            _ => continue,
        };
        return Ok(Some(action));
    }
    Ok(None)
}

/// The version-level part of a snapshot (everything but the level).
pub(crate) fn definition_part(
    s: &Snapshot,
    fallback_reason: &str,
) -> Result<RuleDefinition, String> {
    let t = s.template.as_ref().ok_or("the snapshot has no template")?;
    let mut fields: Vec<(i64, Field)> = Vec::new();
    for (i, f) in s
        .context_fields
        .as_deref()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let raw = f.kind.as_deref().unwrap_or("short_text");
        let kind =
            field_kind(raw).ok_or_else(|| format!("unknown field type {raw:?} in the snapshot"))?;
        let position = f
            .position
            .filter(|p| *p > 0)
            .unwrap_or(i64::try_from(i).unwrap_or(0) + 1);
        fields.push((
            position,
            Field {
                key: FieldKey(f.key.clone()),
                label: f.label.clone().unwrap_or_else(|| f.key.clone()),
                kind,
                required: f.required.unwrap_or(false),
            },
        ));
    }
    fields.sort_by_key(|(p, _)| *p);
    let reason = t
        .reason_template
        .clone()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| fallback_reason.to_owned());
    Ok(RuleDefinition {
        name: t.name.clone().unwrap_or_default(),
        description: String::new(),
        reason,
        appealable: t.appealable.unwrap_or(false),
        counting_window_days: super::current::window(t.case_decay_days.unwrap_or(0))?,
        fields: fields.into_iter().map(|(_, f)| f).collect(),
        levels: Vec::new(),
    }
    .trimmed())
}

/// One rule version being rebuilt from its cases.
#[derive(Clone, Debug)]
pub(crate) struct Rebuild {
    /// The earliest case's version-level part.
    pub base: RuleDefinition,
    /// Levels by `from_case_count`; the earliest case's wins.
    pub levels: BTreeMap<u32, Level>,
    /// Cases whose snapshot disagreed with an earlier one.
    pub conflicts: Vec<String>,
    /// When the earliest case was created.
    pub first_seen: time::OffsetDateTime,
}

impl Rebuild {
    /// Starts from the earliest case (callers feed cases in creation order).
    pub(crate) fn new(base: RuleDefinition, level: Level, at: time::OffsetDateTime) -> Self {
        let mut levels = BTreeMap::new();
        levels.insert(level.from_case_count, level);
        Self {
            base,
            levels,
            conflicts: Vec::new(),
            first_seen: at,
        }
    }

    /// Adds a later case's snapshot.
    pub(crate) fn add(&mut self, part: &RuleDefinition, level: Level, case: &str) {
        let mut conflict = *part != self.base;
        match self.levels.get(&level.from_case_count) {
            Some(existing) => conflict |= *existing != level,
            None => {
                self.levels.insert(level.from_case_count, level);
            }
        }
        if conflict {
            self.conflicts.push(case.to_owned());
        }
    }

    /// The rebuilt definition: the base with every level seen, ascending.
    pub(crate) fn definition(&self) -> RuleDefinition {
        RuleDefinition {
            levels: self.levels.values().cloned().collect(),
            ..self.base.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use quack_core::rule::Strictness;
    use serde_json::json;

    fn snap(trigger: i64, default: bool, action: &serde_json::Value) -> Snapshot {
        Snapshot::parse(&json!({
            "template": {"id": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "name": "Spam", "version": 1,
                         "reason_template": "No spam", "case_decay_days": 0, "appealable": true},
            "selected_level": {"position": 2, "is_default": default, "trigger_case_count": trigger,
                               "notify_user": true, "matched_case_count": 3},
            "actions": [action],
            "context_fields": [{"key": "where", "label": "Where", "type": "discord_message_link", "position": 1, "required": true}],
            "context_values": []
        }))
    }

    #[test]
    fn decisions_read_the_level_and_action() {
        let d = decision(&snap(
            3,
            false,
            &json!({"action_type": "timeout_user", "timeout_duration_seconds": 60}),
        ))
        .unwrap();
        assert_eq!(d.level.from_case_count, 3);
        assert_eq!(
            d.level.action,
            Some(LevelAction::Timeout {
                duration_seconds: 60
            })
        );
        assert_eq!((d.position, d.case_count), (Some(2), 3));
        let d = decision(&snap(0, true, &json!({"action_type": "ban_user"}))).unwrap();
        assert_eq!(d.level.from_case_count, 1);
        assert_eq!(
            d.level.action,
            Some(LevelAction::Ban {
                delete_message_seconds: 0
            })
        );
        let d = decision(&snap(2, false, &json!({"action_type": "send_dm"}))).unwrap();
        assert_eq!(d.level.action, None);
        assert!(decision(&Snapshot::default()).is_err());
        assert!(decision(&snap(2, false, &json!({"action_type": "timeout_user"}))).is_err());
    }

    #[test]
    fn the_v4_variant_and_garbage_read_as_empty() {
        let s = Snapshot::parse(&json!({"historical": true, "v4_action_type": "ban"}));
        assert!(s.template.is_none() && s.selected_level.is_none());
        assert!(Snapshot::parse(&json!([1, 2])).template.is_none());
    }

    #[test]
    fn definition_parts_map_field_types() {
        let part = definition_part(&snap(1, true, &json!({})), "fallback").unwrap();
        assert_eq!(
            part.fields[0].kind,
            quack_core::rule::FieldKind::MessageLink
        );
        assert_eq!(part.reason, "No spam");
        assert_eq!(part.counting_window_days, None);
    }

    fn level(from: u32, dm: bool) -> Level {
        Level {
            from_case_count: from,
            action: None,
            send_dm: dm,
        }
    }

    proptest! {
        /// Whatever order and mix of levels the cases show, the rebuilt
        /// version has strictly increasing levels, keeps the earliest
        /// case's level for each count, flags every disagreeing case, and
        /// passes the reconstructed-import validation.
        #[test]
        fn rebuilt_versions_are_valid(levels in proptest::collection::vec((1u32..20, any::<bool>()), 1..30)) {
            let base = definition_part(&snap(1, true, &json!({})), "r").unwrap();
            let mut r = Rebuild::new(base.clone(), level(levels[0].0, levels[0].1), time::OffsetDateTime::UNIX_EPOCH);
            let mut first: BTreeMap<u32, bool> = BTreeMap::new();
            first.insert(levels[0].0, levels[0].1);
            let mut expected_conflicts = 0;
            for (i, (from, dm)) in levels.iter().enumerate().skip(1) {
                let seen = *first.entry(*from).or_insert(*dm);
                if seen != *dm { expected_conflicts += 1; }
                r.add(&base, level(*from, *dm), &i.to_string());
            }
            let def = r.definition();
            prop_assert!(def.levels.windows(2).all(|w| w[0].from_case_count < w[1].from_case_count));
            for l in &def.levels {
                prop_assert_eq!(Some(&l.send_dm), first.get(&l.from_case_count));
            }
            prop_assert_eq!(r.conflicts.len(), expected_conflicts);
            let valid = def.validate(Strictness::Imported { reconstructed: true }).is_ok();
            prop_assert!(valid);
        }
    }
}
