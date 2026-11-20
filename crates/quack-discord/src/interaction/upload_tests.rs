//! Evidence interactions: ATTACHMENT options, a modal's File Upload, and
//! the "Add as evidence" message command (§7.1, §11.1).

use quack_core::ids::{AttachmentId, CaseId};
use quack_telemetry::labels::InteractionKind;
use quack_text::actions::InputId;
use serde_json::{Value, json};
use uuid::Uuid;

use super::*;
use crate::custom_id::{ComponentId, CustomId};
use crate::fixtures;

fn decoded(kind: u8, data: &Value) -> Interaction {
    decode_json(&fixtures::interaction(kind, data).to_string()).unwrap()
}

/// A resolved attachment as Discord sends it.
fn attachment(id: u64, name: &str, content_type: Option<&str>) -> Value {
    let url = format!("https://cdn.discordapp.com/ephemeral-attachments/5/{id}/{name}");
    let mut a = json!({
        "id": id.to_string(),
        "filename": name,
        "size": 2048,
        "url": url,
        "proxy_url": url.replace("cdn.discordapp.com", "media.discordapp.net"),
        "ephemeral": true
    });
    if let Some(t) = content_type {
        a["content_type"] = json!(t);
    }
    a
}

fn upload(id: u64, name: &str, content_type: Option<&str>) -> Upload {
    Upload {
        attachment_id: AttachmentId::new(id).unwrap(),
        filename: name.to_owned(),
        content_type: content_type.map(str::to_owned),
        size_bytes: 2048,
        url: format!("https://cdn.discordapp.com/ephemeral-attachments/5/{id}/{name}"),
    }
}

fn case_sub(sub: &str, options: &Value, attachments: &Value) -> Value {
    json!({
        "id": "100",
        "name": "case",
        "type": 1,
        "options": [{"name": sub, "type": 1, "options": options}],
        "resolved": {
            "users": {"9": fixtures::user(9, "member")},
            "members": {"9": fixtures::resolved_member(&[21])},
            "attachments": attachments
        }
    })
}

#[test]
fn case_add_reads_its_file_from_the_resolved_attachments() {
    let rule = Uuid::from_u128(77);
    let i = decoded(
        2,
        &case_sub(
            "add",
            &json!([
                {"name": "member", "type": 6, "value": "9"},
                {"name": "rule", "type": 3, "value": rule.to_string()},
                {"name": "file", "type": 11, "value": "300"}
            ]),
            &json!({"300": attachment(300, "proof.png", Some("image/png"))}),
        ),
    );
    let Request::CaseAdd { file, .. } = &i.request else {
        panic!("expected /case add, got {:?}", i.request);
    };
    assert_eq!(*file, Some(upload(300, "proof.png", Some("image/png"))));

    // A file Discord didn't resolve is never dropped silently.
    let unresolved = decoded(
        2,
        &case_sub(
            "add",
            &json!([
                {"name": "member", "type": 6, "value": "9"},
                {"name": "rule", "type": 3, "value": rule.to_string()},
                {"name": "file", "type": 11, "value": "301"}
            ]),
            &json!({}),
        ),
    );
    assert_eq!(
        unresolved.request,
        Request::Unknown(UnknownReason::UnknownCommand)
    );
}

#[test]
fn case_evidence_reads_links_files_and_note() {
    let i = decoded(
        2,
        &case_sub(
            "evidence",
            &json!([
                {"name": "number", "type": 4, "value": 12},
                {"name": "link", "type": 3, "value": " https://discord.com/channels/1/5/10  https://discord.com/channels/1/5/11 "},
                {"name": "file2", "type": 11, "value": "302"},
                {"name": "file", "type": 11, "value": "301"},
                {"name": "note", "type": 3, "value": "From a ticket."}
            ]),
            &json!({
                "301": attachment(301, "a.png", Some("image/png")),
                "302": attachment(302, "b.txt", None)
            }),
        ),
    );
    assert_eq!(
        i.request,
        Request::CaseEvidence {
            number: 12,
            links: vec![
                "https://discord.com/channels/1/5/10".to_owned(),
                "https://discord.com/channels/1/5/11".to_owned()
            ],
            files: vec![
                upload(301, "a.png", Some("image/png")),
                upload(302, "b.txt", None)
            ],
            note: Some("From a ticket.".to_owned()),
        }
    );
    assert_eq!(i.command_name().to_string(), "case evidence");

    let bare = decoded(
        2,
        &case_sub(
            "evidence",
            &json!([{"name": "number", "type": 4, "value": 3}]),
            &json!({}),
        ),
    );
    assert_eq!(
        bare.request,
        Request::CaseEvidence {
            number: 3,
            links: Vec::new(),
            files: Vec::new(),
            note: None,
        }
    );
    let missing = decoded(
        2,
        &case_sub(
            "evidence",
            &json!([
                {"name": "number", "type": 4, "value": 3},
                {"name": "file3", "type": 11, "value": "303"}
            ]),
            &json!({}),
        ),
    );
    assert_eq!(
        missing.request,
        Request::Unknown(UnknownReason::UnknownCommand)
    );
}

#[test]
fn add_as_evidence_is_a_message_command() {
    let mut message = fixtures::message(10, 5);
    message["author"] = fixtures::user(9, "author");
    let i = decoded(
        2,
        &json!({
            "id": "104", "name": "Add as evidence", "type": 3, "target_id": "10",
            "resolved": {"messages": {"10": message}}
        }),
    );
    let Request::AddAsEvidence { message } = &i.request else {
        panic!("expected Add as evidence, got {:?}", i.request);
    };
    assert_eq!((message.id.get(), message.channel_id.get()), (10, 5));
    assert_eq!(message.author.as_ref().map(|a| a.id.get()), Some(9));
    assert_eq!(i.telemetry_kind(), InteractionKind::MessageCommand);
    assert_eq!(i.command_name().to_string(), "Add as evidence");
}

/// The add-evidence modal's File Upload sends attachment ids; the files
/// are in the modal data's `resolved.attachments` (§11.11).
#[test]
fn a_modal_file_upload_reads_its_resolved_attachments_in_order() {
    let case = CaseId::from_uuid(Uuid::from_u128(4));
    let i = decoded(
        5,
        &json!({
            "custom_id": format!("1:eam:{}", case.to_u22()),
            "components": [
                {"type": 18, "id": 1, "component":
                    {"type": 4, "id": 2, "custom_id": "links", "value": ""}},
                {"type": 18, "id": 3, "component":
                    {"type": 19, "id": 4, "custom_id": "files", "values": ["402", "401", "499"]}},
                {"type": 18, "id": 5, "component":
                    {"type": 4, "id": 6, "custom_id": "note", "value": "x"}}
            ],
            "resolved": {"attachments": {
                "401": attachment(401, "one.png", Some("image/png")),
                "402": attachment(402, "two.mp4", Some("video/mp4"))
            }}
        }),
    );
    let Request::ModalSubmit { id, fields, .. } = &i.request else {
        panic!("expected a modal submit, got {:?}", i.request);
    };
    assert_eq!(*id, ComponentId::Current(CustomId::AddEvidenceModal(case)));
    assert_eq!(
        *fields,
        vec![
            (InputId::Links, FieldValue::Text(String::new())),
            (
                InputId::Files,
                FieldValue::Files(vec![
                    upload(402, "two.mp4", Some("video/mp4")),
                    upload(401, "one.png", Some("image/png")),
                ])
            ),
            (InputId::Note, FieldValue::Text("x".into())),
        ]
    );

    // No files chosen: an empty list (the field is optional).
    let empty = decoded(
        5,
        &json!({
            "custom_id": format!("1:eam:{}", case.to_u22()),
            "components": [
                {"type": 18, "id": 3, "component":
                    {"type": 19, "id": 4, "custom_id": "files", "values": []}}
            ]
        }),
    );
    let Request::ModalSubmit { fields, .. } = &empty.request else {
        panic!("expected a modal submit, got {:?}", empty.request);
    };
    assert_eq!(
        *fields,
        vec![(InputId::Files, FieldValue::Files(Vec::new()))]
    );
}
