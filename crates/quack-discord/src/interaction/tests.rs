use quack_core::ids::{CaseId, ChannelId, RoleId, RuleId};
use quack_core::permissions::Permissions;
use quack_telemetry::labels::InteractionKind;
use quack_text::actions::{InputId, SetupArea, SetupOp};
use quack_text::help::Topic;
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::custom_id::{ComponentId, CustomId};
use crate::fixtures;

fn decoded(kind: u8, data: &Value) -> Interaction {
    decode_json(&fixtures::interaction(kind, data).to_string()).unwrap()
}

fn case(sub: &str, options: &Value) -> Value {
    json!({
        "id": "100",
        "name": "case",
        "type": 1,
        "options": [{"name": sub, "type": 1, "options": options}],
        "resolved": {
            "users": {"9": fixtures::user(9, "member")},
            "members": {"9": fixtures::resolved_member(&[21])}
        }
    })
}

fn slash(name: &str, options: &Value) -> Value {
    json!({"id": "101", "name": name, "type": 1, "options": options})
}

fn case_id(n: u128) -> CaseId {
    CaseId::from_uuid(Uuid::from_u128(n))
}

#[test]
fn case_add_carries_the_invoker_member_rule_and_privacy() {
    let rule = Uuid::from_u128(77);
    let i = decoded(
        2,
        &case(
            "add",
            &json!([
                {"name": "member", "type": 6, "value": "9"},
                {"name": "rule", "type": 3, "value": rule.to_string()},
                {"name": "private", "type": 5, "value": true}
            ]),
        ),
    );
    assert_eq!(i.id, 1_310_000_000_000_000_000);
    assert_eq!(i.token.expose(), "interaction-token");
    assert_eq!(i.guild_id.map(quack_core::ids::GuildId::get), Some(1));
    assert_eq!(i.channel_id.map(ChannelId::get), Some(5));
    assert_eq!(i.app_permissions, Some(Permissions(1024)));
    assert!(i.ephemeral_allowed());
    let invoker = i.invoker.clone().unwrap();
    assert_eq!(invoker.account.id.get(), 7);
    let member = invoker.member.unwrap();
    assert_eq!(member.role_ids, vec![RoleId::new(20).unwrap()]);
    assert_eq!(member.permissions, Some(Permissions(8)));
    assert_eq!(member.nick.as_deref(), Some("Mod"));
    let Request::CaseAdd {
        member,
        rule: arg,
        private,
    } = &i.request
    else {
        panic!("expected /case add, got {:?}", i.request);
    };
    assert_eq!(member.account.id.get(), 9);
    assert_eq!(
        member.member.as_ref().map(|m| m.role_ids.clone()),
        Some(vec![RoleId::new(21).unwrap()])
    );
    assert_eq!(*arg, RuleArg::Id(RuleId::from_uuid(rule)));
    assert_eq!(*private, Some(true));
    assert_eq!(i.telemetry_kind(), InteractionKind::SlashCommand);
    assert_eq!(i.command_name().to_string(), "case add");
    let ms = i.created_at().unwrap().unix_timestamp_nanos() / 1_000_000;
    assert_eq!(
        ms,
        i128::from((1_310_000_000_000_000_000_u64 >> 22) + 1_420_070_400_000)
    );
}

#[test]
fn typed_rule_text_and_every_other_command() {
    let typed = decoded(
        2,
        &case(
            "add",
            &json!([
                {"name": "member", "type": 6, "value": "9"},
                {"name": "rule", "type": 3, "value": "  Spam "}
            ]),
        ),
    );
    assert!(matches!(
        typed.request,
        Request::CaseAdd { rule: RuleArg::Name(ref n), private: None, .. } if n == "Spam"
    ));
    let number = json!([{"name": "number", "type": 4, "value": 12}]);
    assert_eq!(
        decoded(2, &case("view", &number)).request,
        Request::CaseView { number: 12 }
    );
    assert_eq!(
        decoded(
            2,
            &case(
                "void",
                &json!([
                    {"name": "number", "type": 4, "value": 3},
                    {"name": "reason", "type": 3, "value": "Wrong member"}
                ])
            )
        )
        .request,
        Request::CaseVoid {
            number: 3,
            reason: "Wrong member".into()
        }
    );
    assert!(matches!(
        decoded(2, &case("history", &json!([{"name": "member", "type": 6, "value": "9"}]))).request,
        Request::CaseHistory { member } if member.account.id.get() == 9
    ));
    assert_eq!(
        decoded(2, &case("list", &json!([]))).request,
        Request::CaseList {
            member: None,
            rule: None
        }
    );
    for (name, request) in [
        ("appeals", Request::Appeals),
        ("rules", Request::Rules),
        ("setup", Request::Setup),
        ("help", Request::Help { topic: None }),
    ] {
        assert_eq!(decoded(2, &slash(name, &json!([]))).request, request);
    }
    assert_eq!(
        decoded(
            2,
            &slash(
                "help",
                &json!([{"name": "topic", "type": 3, "value": "appeals"}])
            )
        )
        .request,
        Request::Help {
            topic: Some(Topic::Appeals)
        }
    );
}

#[test]
fn malformed_commands_are_unknown() {
    let unknown = Request::Unknown(UnknownReason::UnknownCommand);
    assert_eq!(decoded(2, &slash("template", &json!([]))).request, unknown);
    // A case number below 1, and a member option nobody resolved.
    let zero = json!([{"name": "number", "type": 4, "value": 0}]);
    assert_eq!(decoded(2, &case("view", &zero)).request, unknown);
    let unresolved = json!([{"name": "member", "type": 6, "value": "404"}]);
    assert_eq!(decoded(2, &case("history", &unresolved)).request, unknown);
    assert_eq!(decoded(2, &case("reverse", &json!([]))).request, unknown);
}

#[test]
fn autocomplete_reads_the_focused_rule_option() {
    for (sub, command) in [
        ("add", AutocompleteFor::CaseAdd),
        ("list", AutocompleteFor::CaseList),
    ] {
        let i = decoded(
            4,
            &case(
                sub,
                &json!([
                    {"name": "member", "type": 6, "value": "9"},
                    {"name": "rule", "type": 3, "value": "spa", "focused": true}
                ]),
            ),
        );
        assert_eq!(
            i.request,
            Request::Autocomplete {
                command,
                query: "spa".into()
            }
        );
        assert_eq!(i.telemetry_kind(), InteractionKind::Autocomplete);
    }
}

#[test]
fn context_menu_commands() {
    let mut message = fixtures::message(10, 5);
    message["author"] = fixtures::user(9, "author");
    let from_message = decoded(
        2,
        &json!({
            "id": "102", "name": "Add case", "type": 3, "target_id": "10",
            "resolved": {"messages": {"10": message}}
        }),
    );
    let Request::AddCaseFromMessage { message } = &from_message.request else {
        panic!(
            "expected the message command, got {:?}",
            from_message.request
        );
    };
    assert_eq!((message.id.get(), message.channel_id.get()), (10, 5));
    assert_eq!(message.author.as_ref().map(|a| a.id.get()), Some(9));
    assert_eq!(
        from_message.telemetry_kind(),
        InteractionKind::MessageCommand
    );
    let for_member = decoded(
        2,
        &json!({
            "id": "103", "name": "Add case", "type": 2, "target_id": "9",
            "resolved": {
                "users": {"9": fixtures::user(9, "member")},
                "members": {"9": fixtures::resolved_member(&[])}
            }
        }),
    );
    assert!(matches!(
        &for_member.request,
        Request::AddCaseForMember { member } if member.account.id.get() == 9
    ));
    assert_eq!(for_member.command_name().to_string(), "Add case");
}

#[test]
fn components_decode_their_id_values_and_source() {
    let mut on = fixtures::message(11, 5);
    on["flags"] = json!(64);
    let mut payload = fixtures::interaction(
        3,
        &json!({
            "custom_id": "1:cvs",
            "component_type": 3,
            "values": [case_id(1).to_u22(), "not-a-case"]
        }),
    );
    payload["message"] = on;
    let i = decode_json(&payload.to_string()).unwrap();
    assert_eq!(
        i.request,
        Request::Component {
            id: ComponentId::Current(CustomId::CasePicker),
            values: vec![SelectChoice::Case(case_id(1))],
            source: Source {
                message_id: Some(quack_core::ids::MessageId::new(11).unwrap()),
                ephemeral: true
            }
        }
    );
    let channel = decoded(
        3,
        &json!({"custom_id": "1:su:log:ch", "component_type": 8, "values": ["30"]}),
    );
    assert!(matches!(
        channel.request,
        Request::Component {
            id: ComponentId::Current(CustomId::Setup(SetupArea::Log, SetupOp::Channel)),
            ref values,
            source: Source { message_id: None, ephemeral: false },
        } if *values == vec![SelectChoice::Channel(ChannelId::new(30).unwrap())]
    ));
    let legacy = decoded(
        3,
        &json!({"custom_id": "ticket:open:v1:", "component_type": 2}),
    );
    assert!(matches!(
        legacy.request,
        Request::Component {
            id: ComponentId::Legacy(_),
            ..
        }
    ));
    let expired = decoded(3, &json!({"custom_id": "1:zz:1", "component_type": 2}));
    assert!(matches!(
        expired.request,
        Request::Component {
            id: ComponentId::Expired,
            ..
        }
    ));
}

#[test]
fn modal_submits_read_labels_text_and_yes_no() {
    let i = decoded(
        5,
        &json!({
            "custom_id": format!("1:vdm:{}", case_id(3).to_u22()),
            "components": [
                {"type": 18, "id": 1, "component":
                    {"type": 4, "id": 2, "custom_id": "reason", "value": "Wrong member"}},
                {"type": 18, "id": 3, "component":
                    {"type": 3, "id": 4, "custom_id": "f:repeat", "values": ["no"]}},
                {"type": 18, "id": 5, "component":
                    {"type": 3, "id": 6, "custom_id": "f:other", "values": []}},
                {"type": 1, "id": 7, "components": [
                    {"type": 4, "id": 8, "custom_id": "links", "value": ""}]},
                {"type": 18, "id": 9, "component":
                    {"type": 4, "id": 10, "custom_id": "somebody-else", "value": "x"}}
            ]
        }),
    );
    let Request::ModalSubmit { id, fields, source } = &i.request else {
        panic!("expected a modal submit, got {:?}", i.request);
    };
    assert_eq!(
        *id,
        ComponentId::Current(CustomId::VoidCaseModal(case_id(3)))
    );
    assert_eq!(*source, Source::default());
    assert_eq!(
        *fields,
        vec![
            (InputId::Reason, FieldValue::Text("Wrong member".into())),
            (
                InputId::Field(quack_core::rule::FieldKey("repeat".into())),
                FieldValue::YesNo(Some(false))
            ),
            (
                InputId::Field(quack_core::rule::FieldKey("other".into())),
                FieldValue::YesNo(None)
            ),
            (InputId::Links, FieldValue::Text(String::new())),
        ]
    );
    assert_eq!(i.telemetry_kind(), InteractionKind::Modal);
}

#[test]
fn pings_dms_and_payloads_twilight_rejects() {
    let mut ping = fixtures::interaction(1, &Value::Null);
    ping.as_object_mut().unwrap().remove("data");
    assert_eq!(
        decode_json(&ping.to_string()).unwrap().request,
        Request::Unknown(UnknownReason::UnsupportedType)
    );
    let mut dm = fixtures::interaction(3, &json!({"custom_id": "1:to", "component_type": 2}));
    let object = dm.as_object_mut().unwrap();
    object.remove("member");
    object.remove("guild_id");
    object.insert("user".into(), fixtures::user(7, "member"));
    let dm = decode_json(&dm.to_string()).unwrap();
    assert!(!dm.ephemeral_allowed());
    assert!(dm.invoker.as_ref().unwrap().member.is_none());
    let mut lenient = fixtures::interaction(2, &slash("rules", &json!([])));
    let object = lenient.as_object_mut().unwrap();
    object.remove("attachment_size_limit");
    object.remove("authorizing_integration_owners");
    let lenient = lenient.to_string();
    assert!(serde_json::from_str::<RawInteraction>(&lenient).is_err());
    assert_eq!(decode_json(&lenient).unwrap().request, Request::Rules);
    let frame = format!(r#"{{"op":0,"s":3,"t":"INTERACTION_CREATE","d":{lenient}}}"#);
    assert_eq!(recover_dispatch(&frame).unwrap().request, Request::Rules);
    assert!(recover_dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","d":{}}"#).is_none());
    assert!(decode_json("{").is_none());
    assert!(decode_json(r#"{"id": "1"}"#).is_none());
}

#[test]
fn an_interaction_with_unreadable_data_is_still_answerable() {
    // A nested modal component Twilight can't read, and a wrong-typed
    // option: the id, token, and type are enough to answer.
    let modal = fixtures::interaction(
        5,
        &json!({"custom_id": "1:x", "components": [{"type": "?"}]}),
    );
    let ix = decode_json(&modal.to_string()).expect("answerable");
    assert_eq!(ix.id, 1_310_000_000_000_000_000);
    assert_eq!(
        ix.request,
        Request::Unknown(UnknownReason::Malformed {
            autocomplete: false
        })
    );
    assert_eq!(ix.guild_id.map(quack_core::ids::GuildId::get), Some(1));
    let auto = fixtures::interaction(
        4,
        &json!({"id": "101", "name": "case", "type": 1, "options": [{"name": "add", "type": "x"}]}),
    );
    let ix = decode_json(&auto.to_string()).expect("answerable");
    assert_eq!(
        ix.request,
        Request::Unknown(UnknownReason::Malformed { autocomplete: true })
    );
    assert!(decode_json(r#"{"type": 2}"#).is_none(), "no id or token");
}
