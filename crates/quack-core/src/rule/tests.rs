use serde_json::json;
use time::macros::datetime;

use super::*;

fn spam() -> RuleDefinition {
    serde_json::from_value(json!({
        "name": "Spam",
        "description": "Unsolicited ads, link spam, and repeated messages.",
        "reason": "Posting spam or unsolicited advertising.",
        "appealable": true,
        "counting_window_days": 30,
        "fields": [
            { "key": "what_happened", "label": "What happened?", "kind": "long_text", "required": true },
            { "key": "message", "label": "Message link", "kind": "message_link", "required": false }
        ],
        "levels": [
            { "from_case_count": 1, "action": null, "send_dm": true },
            { "from_case_count": 3, "action": { "kind": "timeout", "duration_seconds": 3600 }, "send_dm": true },
            { "from_case_count": 5, "action": { "kind": "ban", "delete_message_seconds": 86400 }, "send_dm": true }
        ]
    }))
    .unwrap()
}

fn field(key: &str) -> Field {
    Field {
        key: FieldKey(key.to_owned()),
        label: "Label".to_owned(),
        kind: FieldKind::ShortText,
        required: false,
    }
}

fn level(from: u32, action: Option<LevelAction>) -> Level {
    Level {
        from_case_count: from,
        action,
        send_dm: false,
    }
}

fn problems(def: &RuleDefinition) -> Vec<RuleProblem> {
    def.validate(Strictness::New).err().unwrap_or_default()
}

#[test]
fn design_example_parses_validates_and_round_trips() {
    let def = spam();
    assert_eq!(def.validate(Strictness::New), Ok(()));
    assert_eq!(
        def.levels[1].action,
        Some(LevelAction::Timeout {
            duration_seconds: 3600
        })
    );
    let again: RuleDefinition =
        serde_json::from_value(serde_json::to_value(&def).unwrap()).unwrap();
    assert_eq!(again, def);
    assert_eq!(
        serde_json::to_value(LevelAction::Kick).unwrap(),
        json!({ "kind": "kick" })
    );
}

#[test]
fn starter_and_honeypot_rules_are_valid() {
    assert_eq!(starter_rule().validate(Strictness::New), Ok(()));
    assert_eq!(honeypot_rule().validate(Strictness::New), Ok(()));
    assert_eq!(honeypot_rule().check_honeypot_compatible(), Ok(()));
    assert_eq!(
        spam().check_honeypot_compatible(),
        Err(DomainError::RequiredFieldOnHoneypotRule)
    );
}

#[test]
fn unknown_json_fields_are_rejected() {
    let mut v = serde_json::to_value(spam()).unwrap();
    v["slug"] = json!("spam");
    assert!(serde_json::from_value::<RuleDefinition>(v).is_err());
    let bad_action =
        json!({ "from_case_count": 1, "action": { "kind": "kick", "x": 1 }, "send_dm": true });
    assert!(serde_json::from_value::<Level>(bad_action).is_err());
    for bad in [
        json!({ "kind": "mute" }),
        json!({ "kind": "timeout" }),
        json!({ "kind": "ban" }),
        json!({ "kind": "timeout", "duration_seconds": 1, "delete_message_seconds": 0 }),
        json!({ "kind": "kick", "duration_seconds": 1 }),
        // A key that doesn't belong to the kind is refused even as null.
        json!({ "kind": "kick", "duration_seconds": null }),
        json!({ "kind": "kick", "delete_message_seconds": null }),
        json!({ "kind": "timeout", "duration_seconds": 60, "delete_message_seconds": null }),
        json!({ "kind": "ban", "delete_message_seconds": 0, "duration_seconds": null }),
        json!({ "kind": "timeout", "duration_seconds": null }),
        json!({ "kind": "ban", "delete_message_seconds": null }),
    ] {
        assert!(
            serde_json::from_value::<LevelAction>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn text_limits() {
    type Case = (fn(&mut RuleDefinition), RuleProblem);
    let cases: &[Case] = &[
        (|d| d.name = "   ".into(), RuleProblem::NameLength),
        (|d| d.name = "x".repeat(101), RuleProblem::NameLength),
        (
            |d| d.description = "x".repeat(501),
            RuleProblem::DescriptionLength,
        ),
        (|d| d.reason = String::new(), RuleProblem::ReasonLength),
        (|d| d.reason = "x".repeat(1001), RuleProblem::ReasonLength),
        (
            |d| d.counting_window_days = Some(0),
            RuleProblem::CountingWindow,
        ),
        (
            |d| d.counting_window_days = Some(36_501),
            RuleProblem::CountingWindow,
        ),
    ];
    for (edit, expected) in cases {
        let mut def = spam();
        edit(&mut def);
        assert_eq!(problems(&def), vec![*expected]);
    }
    let mut edge = spam();
    edge.name = format!("  {}  ", "x".repeat(100));
    edge.description = "x".repeat(500);
    edge.reason = "x".repeat(1000);
    edge.counting_window_days = Some(36_500);
    assert_eq!(edge.validate(Strictness::New), Ok(()));
    edge.counting_window_days = None;
    assert_eq!(edge.validate(Strictness::New), Ok(()));
}

#[test]
fn field_rules() {
    let mut def = spam();
    def.fields = (0..11).map(|i| field(&format!("f{i}"))).collect();
    assert_eq!(problems(&def), vec![RuleProblem::TooManyFields]);

    for bad in ["a", "A1", "_a", "-a", "a b", "a.b", "é1", &"a".repeat(65)] {
        def.fields = vec![field(bad)];
        assert_eq!(
            problems(&def),
            vec![RuleProblem::FieldKey { index: 0 }],
            "{bad:?}"
        );
    }
    for good in ["ab", "0a", "a_b-c", &"a".repeat(64)] {
        def.fields = vec![field(good)];
        assert_eq!(def.validate(Strictness::New), Ok(()), "{good:?}");
    }

    def.fields = vec![field("ab"), field("cd"), field("ab")];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::FieldKeyDuplicate { index: 2 }]
    );
    // An invalid key used twice is both invalid and a duplicate.
    def.fields = vec![field("!"), field("!")];
    assert_eq!(
        problems(&def),
        vec![
            RuleProblem::FieldKey { index: 0 },
            RuleProblem::FieldKey { index: 1 },
            RuleProblem::FieldKeyDuplicate { index: 1 }
        ]
    );

    let mut unlabeled = field("ab");
    unlabeled.label = " ".into();
    def.fields = vec![unlabeled];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::FieldLabelLength { index: 0 }]
    );
}

#[test]
fn level_rules() {
    let mut def = spam();
    def.levels = vec![];
    assert_eq!(problems(&def), vec![RuleProblem::LevelCount]);
    def.levels = (0..11).map(|i| level(i + 1, None)).collect();
    assert_eq!(problems(&def), vec![RuleProblem::LevelCount]);

    def.levels = vec![level(2, None)];
    assert_eq!(problems(&def), vec![RuleProblem::FirstLevelNotOne]);
    def.levels = vec![level(1, None), level(1_000_001, None)];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::LevelCaseCount { index: 1 }]
    );
    // A later threshold of 1 is both out of range and out of order.
    def.levels = vec![level(1, None), level(1, None)];
    assert_eq!(
        problems(&def),
        vec![
            RuleProblem::LevelCaseCount { index: 1 },
            RuleProblem::LevelOrder { index: 1 }
        ]
    );
    def.levels = vec![
        level(1, None),
        level(5, None),
        level(5, None),
        level(4, None),
    ];
    assert_eq!(
        problems(&def),
        vec![
            RuleProblem::LevelOrder { index: 2 },
            RuleProblem::LevelOrder { index: 3 }
        ]
    );

    def.levels = vec![level(
        1,
        Some(LevelAction::Timeout {
            duration_seconds: 0,
        }),
    )];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::TimeoutDuration { index: 0 }]
    );
    def.levels = vec![level(
        1,
        Some(LevelAction::Timeout {
            duration_seconds: 2_419_201,
        }),
    )];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::TimeoutDuration { index: 0 }]
    );
    def.levels = vec![level(
        1,
        Some(LevelAction::Ban {
            delete_message_seconds: 604_801,
        }),
    )];
    assert_eq!(
        problems(&def),
        vec![RuleProblem::BanDeleteWindow { index: 0 }]
    );

    def.levels = vec![
        level(
            1,
            Some(LevelAction::Timeout {
                duration_seconds: 2_419_200,
            }),
        ),
        level(
            1_000_000,
            Some(LevelAction::Ban {
                delete_message_seconds: 604_800,
            }),
        ),
    ];
    assert_eq!(def.validate(Strictness::New), Ok(()));
}

fn imported(def: &RuleDefinition, reconstructed: bool) -> Vec<RuleProblem> {
    def.validate(Strictness::Imported { reconstructed })
        .err()
        .unwrap_or_default()
}

#[test]
fn imported_checks_structure_but_not_size_limits() {
    let mut def = spam();
    def.name = "x".repeat(300);
    def.description = "x".repeat(900);
    def.reason = "x".repeat(2000);
    def.counting_window_days = Some(0);
    def.fields = (0..12).map(|i| field(&format!("F {i}"))).collect();
    def.levels = (1..=12)
        .map(|i| {
            level(
                i * 2_000_000 - 1_999_999,
                Some(LevelAction::Ban {
                    delete_message_seconds: 9_999_999,
                }),
            )
        })
        .collect();
    assert_eq!(imported(&def, false), vec![]);
    assert_ne!(problems(&def), vec![]);

    let mut broken = spam();
    broken.name = "  ".into();
    broken.reason = String::new();
    broken.fields = vec![field("a"), field("a")];
    broken.levels = vec![
        level(
            1,
            Some(LevelAction::Timeout {
                duration_seconds: 2_419_201,
            }),
        ),
        level(3, None),
        level(3, None),
        level(
            2,
            Some(LevelAction::Timeout {
                duration_seconds: 0,
            }),
        ),
    ];
    assert_eq!(
        imported(&broken, false),
        vec![
            RuleProblem::NameLength,
            RuleProblem::ReasonLength,
            RuleProblem::FieldKeyDuplicate { index: 1 },
            RuleProblem::TimeoutDuration { index: 0 },
            RuleProblem::LevelOrder { index: 2 },
            RuleProblem::LevelOrder { index: 3 },
            RuleProblem::TimeoutDuration { index: 3 },
        ]
    );
    broken.levels.clear();
    assert!(imported(&broken, true).contains(&RuleProblem::LevelCount));
}

#[test]
fn only_reconstructed_versions_may_start_above_one() {
    let mut def = spam();
    def.levels = vec![level(3, None), level(5, None)];
    assert_eq!(imported(&def, true), vec![]);
    assert_eq!(imported(&def, false), vec![RuleProblem::FirstLevelNotOne]);
    assert_eq!(problems(&def), vec![RuleProblem::FirstLevelNotOne]);
    def.levels = vec![level(0, None)];
    assert_eq!(
        imported(&def, true),
        vec![RuleProblem::LevelCaseCount { index: 0 }]
    );
    assert_eq!(imported(&def, false), vec![RuleProblem::FirstLevelNotOne]);
}

#[test]
fn a_rule_without_fields_takes_free_text_details() {
    let mut def = spam();
    assert_eq!(def.context_fields().as_ref(), def.fields.as_slice());
    def.fields.clear();
    let fields = def.context_fields();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].key.as_str(), "details");
    assert_eq!(fields[0].label, "Details");
    assert_eq!(fields[0].kind, FieldKind::LongText);
    assert!(!fields[0].required);
    assert!(fields[0].key.is_valid());
    // Never stored: the definition itself keeps no fields.
    assert_eq!(serde_json::to_value(&def).unwrap()["fields"], json!([]));
}

#[test]
fn starter_and_honeypot_texts_match_the_design() {
    let starter = starter_rule();
    assert_eq!(starter.name, "General rule violation");
    assert_eq!(
        starter.description,
        "A starter rule for general violations. Review it and change it for this server."
    );
    assert_eq!(starter.reason, "General rule violation");
    let froms: Vec<u32> = starter.levels.iter().map(|l| l.from_case_count).collect();
    assert_eq!(froms, [1, 3, 5]);
    let honeypot = honeypot_rule();
    assert_eq!(honeypot.name, HONEYPOT_RULE_NAME);
    assert_eq!(
        honeypot.reason,
        "Posted in the honeypot channel despite the warning."
    );
}

#[test]
fn every_problem_is_reported_at_once() {
    let mut def = spam();
    def.name.clear();
    def.reason.clear();
    def.fields = vec![field("A")];
    def.levels = vec![level(
        2,
        Some(LevelAction::Timeout {
            duration_seconds: 0,
        }),
    )];
    assert_eq!(
        problems(&def),
        vec![
            RuleProblem::NameLength,
            RuleProblem::ReasonLength,
            RuleProblem::FieldKey { index: 0 },
            RuleProblem::FirstLevelNotOne,
            RuleProblem::TimeoutDuration { index: 0 },
        ]
    );
}

#[test]
fn problem_paths_and_codes() {
    let cases = [
        (RuleProblem::NameLength, "name", "name_length"),
        (
            RuleProblem::CountingWindow,
            "counting_window_days",
            "counting_window",
        ),
        (
            RuleProblem::FieldKeyDuplicate { index: 2 },
            "fields[2].key",
            "field_key_duplicate",
        ),
        (
            RuleProblem::FieldLabelLength { index: 0 },
            "fields[0].label",
            "field_label_length",
        ),
        (
            RuleProblem::FirstLevelNotOne,
            "levels[0].from_case_count",
            "first_level_not_one",
        ),
        (
            RuleProblem::LevelOrder { index: 1 },
            "levels[1].from_case_count",
            "level_order",
        ),
        (
            RuleProblem::TimeoutDuration { index: 3 },
            "levels[3].action.duration_seconds",
            "timeout_duration",
        ),
        (
            RuleProblem::BanDeleteWindow { index: 0 },
            "levels[0].action.delete_message_seconds",
            "ban_delete_window",
        ),
    ];
    for (problem, path, code) in cases {
        assert_eq!(problem.path(), path);
        assert_eq!(problem.code().as_str(), code);
    }
}

#[test]
fn trimmed_trims_text_but_not_keys() {
    let mut def = spam();
    def.name = "  Spam ".into();
    def.fields[0].label = " Q ".into();
    def.fields[0].key = FieldKey(" k ".into());
    let t = def.trimmed();
    assert_eq!(t.name, "Spam");
    assert_eq!(t.fields[0].label, "Q");
    assert_eq!(t.fields[0].key.as_str(), " k ");
}

#[test]
fn versioning() {
    assert_eq!(next_version(3, 3), Ok(4));
    assert_eq!(next_version(3, 2), Err(DomainError::RuleChanged));
    assert_eq!(
        next_version(u32::MAX, u32::MAX),
        Err(DomainError::RuleChanged)
    );
}

#[test]
fn rule_file_round_trip_and_checks() {
    let file = RuleFile::new(spam(), datetime!(2026-11-17 18:00 UTC));
    let json = serde_json::to_value(&file).unwrap();
    assert_eq!(json["format"], "quack-rule");
    assert_eq!(json["format_version"], 1);
    assert_eq!(json["exported_at"], "2026-11-17T18:00:00Z");
    let parsed: RuleFile = serde_json::from_value(json).unwrap();
    assert_eq!(parsed.clone().into_rule(), Ok(spam()));

    let mut wrong = parsed.clone();
    wrong.format = "quack-template".into();
    assert_eq!(wrong.into_rule(), Err(RuleFileError::WrongFormat));
    let mut future = parsed.clone();
    future.format_version = 2;
    assert_eq!(
        future.into_rule(),
        Err(RuleFileError::UnsupportedVersion(2))
    );
    let mut invalid = parsed;
    invalid.rule.levels.clear();
    assert_eq!(
        invalid.into_rule(),
        Err(RuleFileError::Invalid(vec![RuleProblem::LevelCount]))
    );
}
