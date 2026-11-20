//! Evidence from Discord (§7, §11.1, §11.5, §11.7): the Add evidence modal
//! with files and a note, `/case evidence`, `/case add file` (also through
//! a multi-page draft), the "Add as evidence" message command, and the
//! controls of View evidence: the spoilered gallery, Show to member, the
//! note, Messages around it, and Remove. Every refusal is private, and
//! nothing is saved when it refuses.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use common::payload::{self, Who, interaction, member, moderator};
use common::{Sent, World, snapshot};
use quack_core::case::MessageLink;
use quack_core::evidence::{EvidenceKind, EvidenceState};
use quack_core::ids::{CaseId, ChannelId, EvidenceId, MessageId};
use quack_core::rule::{Field, FieldKey, FieldKind};
use quack_db::evidence::Evidence;
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{GUILD, MEMBER, sf};
use quack_text::actions::InputId;
use quack_text::model::{Block, InputKind};
use serde_json::{Value, json};

type Result = anyhow::Result<()>;

/// A staff-only evidence channel, saved in the settings.
async fn evidence_channel(w: &World) -> anyhow::Result<ChannelId> {
    let channel =
        w.h.fake
            .world(|world| world.add_text_channel(w.h.guild, true));
    w.h.db
        .execute(&format!(
            "UPDATE guild_settings SET evidence_channel_id = {channel} WHERE guild_id = {GUILD}"
        ))
        .await?;
    Ok(channel)
}

/// An interaction attachment the fake CDN serves, as Discord resolves it.
fn attachment(w: &World, name: &str, content_type: &str, bytes: Vec<u8>) -> (String, Value) {
    let size = bytes.len();
    let id = w.h.fake.world(|world| {
        let id = world.next_id();
        let url = format!("https://cdn.discordapp.com/ephemeral-attachments/1/{id}/{name}");
        world.files.insert(url, bytes);
        id
    });
    let url = format!("https://cdn.discordapp.com/ephemeral-attachments/1/{id}/{name}");
    let value = json!({
        "id": id.to_string(),
        "filename": name,
        "content_type": content_type,
        "size": size,
        "url": url,
        "proxy_url": url,
        "ephemeral": true
    });
    (id.to_string(), value)
}

fn png(w: &World) -> (String, Value) {
    attachment(w, "proof.png", "image/png", vec![137, 80, 78, 71])
}

/// A command payload with resolved attachments.
fn with_attachments(mut data: Value, files: &[&(String, Value)]) -> Value {
    let map: serde_json::Map<String, Value> = files
        .iter()
        .map(|(id, v)| (id.clone(), v.clone()))
        .collect();
    if data.get("resolved").is_none() {
        data["resolved"] = json!({});
    }
    data["resolved"]["attachments"] = Value::Object(map);
    data
}

/// The add-evidence modal's submit: links, the File Upload, and a note.
fn evidence_modal(case: CaseId, links: &str, files: &[&(String, Value)], note: &str) -> Value {
    let ids: Vec<&str> = files.iter().map(|(id, _)| id.as_str()).collect();
    let data = json!({
        "custom_id": format!("1:eam:{}", case.to_u22()),
        "components": [
            {"type": 18, "id": 1, "component":
                {"type": 4, "id": 2, "custom_id": "links", "value": links}},
            {"type": 18, "id": 3, "component":
                {"type": 19, "id": 4, "custom_id": "files", "values": ids}},
            {"type": 18, "id": 5, "component":
                {"type": 4, "id": 6, "custom_id": "note", "value": note}}
        ]
    });
    with_attachments(data, files)
}

/// A component or modal on an ephemeral message.
fn on_ephemeral(w: &World, who: Who, kind: u8, data: &Value) -> Value {
    payload::on_message(interaction(kind, who, w.channel, data), w.channel, true)
}

fn button(w: &World, who: Who, id: &str) -> Value {
    on_ephemeral(w, who, 3, &payload::component(id, 2, &[]))
}

async fn items(w: &World, case: CaseId) -> anyhow::Result<Vec<Evidence>> {
    let mut conn = w.h.db.db.conn().await?;
    Ok(quack_db::evidence::for_case(&mut conn, w.h.guild, case).await?)
}

async fn evidence_rows(w: &World) -> anyhow::Result<i64> {
    w.h.db.count("SELECT count(*) FROM evidence").await
}

/// A message the member posted in the commands channel, and its link.
fn posted(w: &World, text: &str) -> (MessageId, String) {
    let message =
        w.h.fake
            .world(|world| world.post(w.h.guild, w.channel, sf(MEMBER), text, Vec::new()));
    let link = MessageLink {
        guild_id: w.h.guild,
        channel_id: w.channel,
        message_id: message,
    }
    .to_url();
    (message, link)
}

async fn spam_case(w: &World) -> anyhow::Result<CaseId> {
    let rule = w.h.rule(warning("Spam")).await?;
    Ok(w.h.apply(MEMBER, rule).await?.case.id)
}

#[tokio::test]
async fn the_add_evidence_modal_uploads_files_with_a_note() -> Result {
    let w = World::new().await?;
    let case = spam_case(&w).await?;
    let evidence_copies = evidence_channel(&w).await?;

    // The modal: links (optional), a File Upload of 0–10, a note.
    w.send(&button(&w, moderator(), &format!("1:ea:{}", case.to_u22())))
        .await;
    w.assert_single_ack();
    let modal = w.modal().expect("the add-evidence modal");
    let files = modal.inputs.iter().find(|i| i.id == InputId::Files);
    assert!(
        matches!(
            files.map(|i| &i.kind),
            Some(InputKind::Files { min: 0, max: 10 })
        ),
        "{modal:?}"
    );
    // Someone who isn't staff gets no modal.
    w.send(&button(&w, member(), &format!("1:ea:{}", case.to_u22())))
        .await;
    assert!(w.modal().is_none());

    let file = png(&w);
    let sent = w
        .send(&on_ephemeral(
            &w,
            moderator(),
            5,
            &evidence_modal(case, "", &[&file], "From a ticket."),
        ))
        .await;
    w.assert_acked_first();
    assert_eq!(sent.first(), Some(&Sent::DeferUpdate));
    snapshot("evidence_added_files", &w.answer());
    w.h.settle().await?;

    let found = items(&w, case).await?;
    assert_eq!(found.len(), 1);
    let upload = &found[0];
    assert_eq!(
        (upload.kind, upload.state, upload.visible_to_member),
        (EvidenceKind::Upload, EvidenceState::Saved, false)
    );
    assert_eq!(upload.note.as_deref(), Some("From a ticket."));
    let copied = w.h.fake.world(|world| {
        world
            .messages
            .values()
            .filter(|m| m.fetched.channel_id == evidence_copies)
            .count()
    });
    assert_eq!(copied, 1, "the file was copied to the evidence channel");

    // View evidence: the uploader, the note, and a spoilered gallery.
    w.send(&button(
        &w,
        moderator(),
        &format!("1:ev:{}:1", case.to_u22()),
    ))
    .await;
    w.assert_acked_first();
    let view = w.answer();
    snapshot("evidence_view_upload", &view);
    let gallery = view.blocks.iter().find_map(|b| match b {
        Block::Gallery(items) => Some(items.clone()),
        _ => None,
    });
    let gallery = gallery.expect("a gallery of the copy");
    assert_eq!(gallery.len(), 1);
    assert!(
        gallery
            .iter()
            .all(|g| g.spoiler && g.url.starts_with("https://"))
    );
    Ok(())
}

#[tokio::test]
async fn uploads_are_refused_before_anything_is_saved() -> Result {
    let w = World::new().await?;
    let case = spam_case(&w).await?;
    let file = png(&w);

    // No evidence channel: files can't be copied, so none are taken.
    w.send(&on_ephemeral(
        &w,
        moderator(),
        5,
        &evidence_modal(case, "", &[&file], ""),
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_refused_no_channel", &w.answer());

    // A file type Quack doesn't copy.
    evidence_channel(&w).await?;
    let zip = attachment(&w, "logs.zip", "application/zip", vec![1, 2]);
    w.send(&on_ephemeral(
        &w,
        moderator(),
        5,
        &evidence_modal(case, "", &[&zip, &file], ""),
    ))
    .await;
    snapshot("evidence_refused_file_type", &w.answer());

    // Neither a link nor a file.
    w.send(&on_ephemeral(
        &w,
        moderator(),
        5,
        &evidence_modal(case, " \n ", &[], "A note alone"),
    ))
    .await;
    snapshot("evidence_refused_nothing", &w.answer());

    // Someone who isn't staff.
    w.send(&on_ephemeral(
        &w,
        member(),
        5,
        &evidence_modal(case, "", &[&file], ""),
    ))
    .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    snapshot("evidence_refused_not_staff", &w.answer());
    assert_eq!(evidence_rows(&w).await?, 0, "nothing was saved");
    Ok(())
}

fn case_evidence(options: &Value, files: &[&(String, Value)]) -> Value {
    with_attachments(payload::case("evidence", options, None), files)
}

#[tokio::test]
async fn case_evidence_adds_links_files_and_a_note() -> Result {
    let w = World::new().await?;
    spam_case(&w).await?;
    evidence_channel(&w).await?;
    let (_, link) = posted(&w, "proof");
    let file = png(&w);
    let options = json!([
        {"name": "number", "type": 4, "value": 1},
        {"name": "link", "type": 3, "value": link},
        {"name": "file", "type": 11, "value": file.0},
        {"name": "note", "type": 3, "value": "Both from the ticket."}
    ]);
    let sent = w
        .send(&interaction(
            2,
            moderator(),
            w.channel,
            &case_evidence(&options, &[&file]),
        ))
        .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert_eq!(sent.first(), Some(&Sent::Defer { ephemeral: true }));
    snapshot("case_evidence_added", &w.answer());
    assert_eq!(evidence_rows(&w).await?, 2, "the message and the upload");
    let notes =
        w.h.db
            .count("SELECT count(*) FROM evidence WHERE note = 'Both from the ticket.'")
            .await?;
    assert_eq!(notes, 2, "the note is on every item the call created");

    // A case number that names nothing, nothing to add, and a member.
    let missing = json!([
        {"name": "number", "type": 4, "value": 9},
        {"name": "link", "type": 3, "value": link}
    ]);
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_evidence(&missing, &[]),
    ))
    .await;
    snapshot("case_evidence_not_found", &w.answer());
    let nothing = json!([{"name": "number", "type": 4, "value": 1}]);
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_evidence(&nothing, &[]),
    ))
    .await;
    snapshot("evidence_refused_nothing", &w.answer());
    w.send(&interaction(
        2,
        member(),
        w.channel,
        &case_evidence(&options, &[&file]),
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_evidence_not_staff", &w.answer());
    assert_eq!(evidence_rows(&w).await?, 2);
    Ok(())
}

/// `/case add` with a `file` option.
fn case_add_with_file(rule: quack_core::ids::RuleId, file: &(String, Value)) -> Value {
    let mut data = payload::case_add(MEMBER, rule, None);
    data["options"][0]["options"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "file", "type": 11, "value": file.0}));
    data["resolved"]["attachments"] = json!({ file.0.clone(): file.1.clone() });
    data
}

#[tokio::test]
async fn case_add_file_is_uploaded_with_the_case_or_refused_before_it() -> Result {
    let w = World::new().await?;
    let rule = w.h.rule(warning("Spam")).await?;
    let file = png(&w);

    // No evidence channel: no case.
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_add_with_file(rule, &file),
    ))
    .await;
    w.assert_acked_first();
    snapshot("case_add_file_refused", &w.answer());
    assert_eq!(w.h.db.count("SELECT count(*) FROM cases").await?, 0);

    evidence_channel(&w).await?;
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_add_with_file(rule, &file),
    ))
    .await;
    w.assert_acked_first();
    w.h.settle().await?;
    let case: CaseId = {
        let mut conn = w.h.db.db.conn().await?;
        quack_db::cases::get_by_number(&mut conn, w.h.guild, 1)
            .await?
            .unwrap()
            .id
    };
    let found = items(&w, case).await?;
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].kind, found[0].state),
        (EvidenceKind::Upload, EvidenceState::Saved)
    );
    // The confirmation next to the public receipt offers Add evidence and
    // View evidence, so more can be added without opening the case.
    let edits = w.h.fake.world(|world| world.interaction_edits.clone());
    let posted = edits
        .last()
        .map(|(_, m)| m.clone())
        .expect("the confirmation");
    snapshot("case_add_file_confirmation", &posted);
    Ok(())
}

/// A rule whose form has one required field.
async fn form_rule(w: &World) -> anyhow::Result<quack_core::ids::RuleId> {
    let mut def = warning("Scam");
    def.fields = vec![Field {
        key: FieldKey("what".to_owned()),
        label: "What happened?".to_owned(),
        kind: FieldKind::ShortText,
        required: true,
    }];
    w.h.rule(def).await
}

#[tokio::test]
async fn a_draft_keeps_the_file_until_the_case_is_applied() -> Result {
    let w = World::new().await?;
    let rule = form_rule(&w).await?;
    let file = png(&w);

    // Without an evidence channel the form doesn't open.
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_add_with_file(rule, &file),
    ))
    .await;
    w.assert_single_ack();
    assert!(w.modal().is_none());
    snapshot("case_add_form_no_channel", &w.answer());

    // A file Quack can't copy: no form either.
    evidence_channel(&w).await?;
    let zip = attachment(&w, "logs.zip", "application/zip", vec![1, 2]);
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_add_with_file(rule, &zip),
    ))
    .await;
    assert!(w.modal().is_none());
    snapshot("case_add_form_file_refused", &w.answer());
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM case_drafts WHERE evidence_file IS NOT NULL")
            .await?,
        0
    );

    // A good file: the form opens and the draft keeps the file.
    w.send(&interaction(
        2,
        moderator(),
        w.channel,
        &case_add_with_file(rule, &file),
    ))
    .await;
    w.assert_single_ack();
    let modal = w.modal().expect("the form");
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM case_drafts WHERE evidence_file IS NOT NULL")
            .await?,
        1
    );
    let quack_text::ModalAction::DraftPage(at) = modal.action else {
        panic!("{:?}", modal.action);
    };
    let id = format!(
        "1:cd:{}:{}:{}:{}",
        at.draft.to_u22(),
        at.rule.to_u22(),
        at.version,
        at.page
    );
    w.send(&interaction(
        5,
        moderator(),
        w.channel,
        &payload::modal(&id, &[("f:what", "a fake giveaway")]),
    ))
    .await;
    w.assert_acked_first();
    w.h.settle().await?;
    let uploads =
        w.h.db
            .count("SELECT count(*) FROM evidence WHERE kind = 'upload' AND state = 'saved'")
            .await?;
    assert_eq!(uploads, 1, "the draft's file became the case's upload");
    assert_eq!(w.h.db.count("SELECT count(*) FROM case_drafts").await?, 0);
    Ok(())
}

/// The "Add as evidence" message command on `message` by the member.
fn add_as_evidence(w: &World, message: MessageId) -> Value {
    let mut data = payload::add_case_on(message.get(), w.channel, MEMBER);
    data["name"] = json!("Add as evidence");
    data
}

#[tokio::test]
async fn add_as_evidence_offers_recent_cases_and_a_case_number() -> Result {
    let w = World::new().await?;
    let case = spam_case(&w).await?;
    let (message, _) = posted(&w, "free nitro");
    let sent = w
        .send(&interaction(
            2,
            moderator(),
            w.channel,
            &add_as_evidence(&w, message),
        ))
        .await;
    w.assert_acked_first();
    w.assert_ephemeral();
    assert_eq!(sent.first(), Some(&Sent::Defer { ephemeral: true }));
    snapshot("add_as_evidence_picker", &w.answer());

    w.send(&interaction(
        2,
        member(),
        w.channel,
        &add_as_evidence(&w, message),
    ))
    .await;
    w.assert_acked_first();
    snapshot("add_as_evidence_not_staff", &w.answer());

    // Choosing the case adds the message.
    let select = payload::component(
        &format!("1:exs:{}:{message}", w.channel),
        3,
        &[case.to_u22()],
    );
    let sent = w.send(&on_ephemeral(&w, moderator(), 3, &select)).await;
    w.assert_acked_first();
    assert_eq!(sent.first(), Some(&Sent::DeferUpdate));
    snapshot("add_as_evidence_added", &w.answer());
    assert_eq!(evidence_rows(&w).await?, 1);

    // Enter a case number: the modal, refused for a member.
    let enter = format!("1:exn:{}:{message}", w.channel);
    w.send(&button(&w, moderator(), &enter)).await;
    w.assert_single_ack();
    assert!(w.modal().is_some_and(|m| m.title == "Add as evidence"));
    w.send(&button(&w, member(), &enter)).await;
    assert!(w.modal().is_none());

    let (other, _) = posted(&w, "and again");
    let number = |n: &str| {
        payload::modal(
            &format!("1:exm:{}:{other}", w.channel),
            &[("case", n), ("note", "Same scam.")],
        )
    };
    w.send(&on_ephemeral(&w, moderator(), 5, &number("twelve")))
        .await;
    w.assert_acked_first();
    snapshot("add_as_evidence_bad_number", &w.answer());
    w.send(&on_ephemeral(&w, member(), 5, &number("twelve")))
        .await;
    snapshot("add_as_evidence_not_staff", &w.answer());
    w.send(&on_ephemeral(&w, moderator(), 5, &number("#1")))
        .await;
    snapshot("add_as_evidence_by_number", &w.answer());
    assert_eq!(
        w.h.db
            .count("SELECT count(*) FROM evidence WHERE note = 'Same scam.'")
            .await?,
        1
    );
    Ok(())
}

/// A case with a saved message (with messages around it) and an upload.
async fn case_with_items(w: &World) -> anyhow::Result<(CaseId, EvidenceId, EvidenceId)> {
    let case = spam_case(w).await?;
    evidence_channel(w).await?;
    posted(w, "before the scam");
    let (_, link) = posted(w, "free nitro, click here");
    posted(w, "after the scam");
    let file = png(w);
    w.send(&on_ephemeral(
        w,
        moderator(),
        5,
        &evidence_modal(case, &link, &[&file], ""),
    ))
    .await;
    w.h.settle().await?;
    let found = items(w, case).await?;
    let id_of = |kind| found.iter().find(|e| e.kind == kind).map(|e| e.id).unwrap();
    Ok((
        case,
        id_of(EvidenceKind::Message),
        id_of(EvidenceKind::Upload),
    ))
}

#[tokio::test]
async fn visibility_and_the_note_change_an_item_and_show_it_again() -> Result {
    let w = World::new().await?;
    let (case, message, upload) = case_with_items(&w).await?;
    let show = format!("1:evv:{}:1", upload.to_u22());

    // Someone who isn't staff changes nothing.
    w.send(&button(&w, member(), &show)).await;
    w.assert_acked_first();
    snapshot("evidence_control_not_staff", &w.answer());
    assert!(
        !items(&w, case)
            .await?
            .iter()
            .any(|e| e.id == upload && e.visible_to_member)
    );

    w.send(&button(&w, moderator(), &show)).await;
    w.assert_acked_first();
    snapshot("evidence_shown", &w.answer());
    assert!(
        items(&w, case)
            .await?
            .iter()
            .any(|e| e.id == upload && e.visible_to_member)
    );
    // A saved message is always visible.
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evv:{}:0", message.to_u22()),
    ))
    .await;
    snapshot("evidence_not_upload", &w.answer());

    // Edit note: prefilled at its revision (the documented exception).
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evn:{}", upload.to_u22()),
    ))
    .await;
    w.assert_single_ack();
    let modal = w.modal().expect("the note modal");
    let quack_text::ModalAction::EvidenceNote { revision, .. } = modal.action else {
        panic!("{:?}", modal.action);
    };
    w.send(&button(&w, member(), &format!("1:evn:{}", upload.to_u22())))
        .await;
    assert!(w.modal().is_none());
    let save = payload::modal(
        &format!("1:evnm:{}:{revision}", upload.to_u22()),
        &[("note", "Screenshot from the ticket.")],
    );
    w.send(&on_ephemeral(&w, moderator(), 5, &save)).await;
    w.assert_acked_first();
    snapshot("evidence_note_saved", &w.answer());
    // The same old revision again: someone else changed it.
    w.send(&on_ephemeral(&w, moderator(), 5, &save)).await;
    snapshot("evidence_note_changed", &w.answer());
    let note = items(&w, case)
        .await?
        .into_iter()
        .find(|e| e.id == upload)
        .and_then(|e| e.note);
    assert_eq!(note.as_deref(), Some("Screenshot from the ticket."));

    // The prefilled modal now shows the note.
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evn:{}", upload.to_u22()),
    ))
    .await;
    let value = w.modal().and_then(|m| {
        m.inputs.into_iter().find_map(|i| match i.kind {
            InputKind::Text { value, .. } => value,
            _ => None,
        })
    });
    assert_eq!(value.as_deref(), Some("Screenshot from the ticket."));
    Ok(())
}

#[tokio::test]
async fn messages_around_it_and_removal() -> Result {
    let w = World::new().await?;
    let (case, message, upload) = case_with_items(&w).await?;

    w.send(&button(
        &w,
        moderator(),
        &format!("1:evs:{}", message.to_u22()),
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_surrounding", &w.answer());

    w.send(&button(
        &w,
        moderator(),
        &format!("1:evr:{}", upload.to_u22()),
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_remove_confirm", &w.answer());
    w.send(&button(
        &w,
        member(),
        &format!("1:evrc:{}", upload.to_u22()),
    ))
    .await;
    snapshot("evidence_control_not_staff", &w.answer());
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evrc:{}", upload.to_u22()),
    ))
    .await;
    w.assert_acked_first();
    snapshot("evidence_removed", &w.answer());
    let removed = items(&w, case)
        .await?
        .into_iter()
        .find(|e| e.id == upload)
        .unwrap();
    assert!(removed.is_removed());
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evrc:{}", upload.to_u22()),
    ))
    .await;
    snapshot("evidence_removed_already", &w.answer());

    // A removed item refuses changes and never shows in Discord.
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evv:{}:1", upload.to_u22()),
    ))
    .await;
    snapshot("evidence_removed_refused", &w.answer());
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evn:{}", upload.to_u22()),
    ))
    .await;
    assert!(w.modal().is_none());
    w.send(&button(
        &w,
        moderator(),
        &format!("1:ev:{}:2", case.to_u22()),
    ))
    .await;
    let view = w.answer().plain();
    assert!(view.contains("Evidence 1 of 1"), "{view}");

    // Another server's (or no) item names nothing here.
    let unknown = EvidenceId::from_uuid(uuid::Uuid::now_v7());
    w.send(&button(
        &w,
        moderator(),
        &format!("1:evs:{}", unknown.to_u22()),
    ))
    .await;
    snapshot("evidence_not_found", &w.answer());
    Ok(())
}
