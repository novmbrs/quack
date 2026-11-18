//! What message and interaction calls send: flags, nonces, files, and
//! tokens.

use httpmock::MockServer;
use quack_core::ids::{AccountId, AttachmentId, ChannelId, MessageId};
use quack_text::{Msg, Text, Tone};
use serde_json::json;

use super::client;
use crate::fixtures;
use crate::model::{FileEdit, InteractionRef, InteractionToken, Nonce, OutgoingFile};
use crate::render::{RenderedMessage, render};
use crate::rest::{AutocompleteChoice, MessageBody};

fn c(id: u64) -> ChannelId {
    ChannelId::new(id).unwrap()
}

fn rendered() -> RenderedMessage {
    render(
        &Msg::text(Tone::Success, Text::fixed("Done.")),
        &quack_text::CommandIds::EMPTY,
    )
}

#[tokio::test]
async fn messages_carry_nonce_flags_and_empty_allowed_mentions() {
    let server = MockServer::start_async().await;
    let message = rendered();
    let components = serde_json::to_value(&message.components).unwrap();
    let create = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/channels/5/messages")
                .header("content-type", "application/json")
                .json_body(json!({
                    "components": components,
                    "flags": 32768,
                    "allowed_mentions": {"parse": []},
                    "nonce": "AAAAAAAAAAAAAAAAAAAAAA0",
                    "enforce_nonce": true
                }));
            then.status(200).json_body(fixtures::message(10, 5));
        })
        .await;
    let ping = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/channels/6/messages")
                .json_body_includes(r#"{"allowed_mentions": {"parse": [], "users": ["9"]}}"#);
            then.status(200).json_body(fixtures::message(11, 6));
        })
        .await;
    let client = client(&server.base_url());
    let nonce = Nonce::new("AAAAAAAAAAAAAAAAAAAAAA0").unwrap();
    let sent = client
        .send(
            c(5),
            &MessageBody::rendered(message.clone()),
            &[],
            Some(&nonce),
        )
        .await
        .done()
        .unwrap();
    assert_eq!(sent.message_id, MessageId::new(10).unwrap());
    assert_eq!(
        sent.attachments,
        vec![("SPOILER_a.png".to_owned(), AttachmentId::new(77).unwrap())]
    );
    let body = MessageBody::rendered(message).mentioning(vec![AccountId::new(9).unwrap()]);
    assert!(client.send(c(6), &body, &[], None).await.is_done());
    create.assert_async().await;
    ping.assert_async().await;
}

#[tokio::test]
async fn files_go_as_multipart_and_edits_keep_or_replace_them() {
    let server = MockServer::start_async().await;
    let upload = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/channels/5/messages")
                .header_prefix("content-type", "multipart/form-data; boundary=")
                .body_includes(r#"name="payload_json""#)
                .body_includes(r#""attachments":[{"id":0,"filename":"SPOILER_a.png"}]"#)
                .body_includes(r#""allowed_mentions":{"parse":[]}"#)
                .body_includes(r#"name="files[0]"; filename="SPOILER_a.png""#)
                .body_includes("PNGDATA")
                .body_includes("Content-Type: image/png")
                .body_excludes("components");
            then.status(200).json_body(fixtures::message(10, 5));
        })
        .await;
    let keep = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/channels/5/messages/10")
                .json_body_includes(r#"{"attachments": [{"id": "77"}], "flags": 32768}"#);
            then.status(200).json_body(fixtures::message(10, 5));
        })
        .await;
    let client = client(&server.base_url());
    let file = OutgoingFile {
        name: "SPOILER_a.png".into(),
        content_type: Some("image/png".into()),
        bytes: b"PNGDATA".to_vec(),
    };
    let nonce = Nonce::new("file").unwrap();
    assert!(
        client
            .send(c(5), &MessageBody::files_only(), &[file], Some(&nonce))
            .await
            .is_done()
    );
    let edit = client
        .edit(
            c(5),
            MessageId::new(10).unwrap(),
            &MessageBody::rendered(rendered()),
            &FileEdit::Keep(AttachmentId::new(77).unwrap()),
        )
        .await;
    assert!(edit.is_done());
    upload.assert_async().await;
    keep.assert_async().await;
}

#[tokio::test]
async fn interaction_responses_use_the_token_and_no_bot_auth() {
    let server = MockServer::start_async().await;
    let path = "/api/v10/interactions/1/tok/callback";
    let defer = server
        .mock_async(|when, then| {
            when.method("POST")
                .path(path)
                .header_missing("authorization")
                .json_body(json!({"type": 5, "data": {"flags": 64}}));
            then.status(204);
        })
        .await;
    let update = server
        .mock_async(|when, then| {
            when.method("POST").path(path).json_body(json!({"type": 6}));
            then.status(204);
        })
        .await;
    let choices = server
        .mock_async(|when, then| {
            when.method("POST").path(path).json_body(
                json!({"type": 8, "data": {"choices": [{"name": "Spam", "value": "u"}]}}),
            );
            then.status(204);
        })
        .await;
    let modal = server
        .mock_async(|when, then| {
            when.method("POST")
                .path(path)
                .json_body_includes(r#"{"type": 9}"#)
                .body_includes(r#""title":"Void case #4""#)
                .body_includes(r#""custom_id":"1:vdm:"#);
            then.status(204);
        })
        .await;
    let original = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/webhooks/42/tok/messages/@original")
                .header_missing("authorization")
                .json_body_includes(r#"{"flags": 32768, "allowed_mentions": {"parse": []}}"#);
            then.status(200).json_body(fixtures::message(12, 5));
        })
        .await;
    let followup = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/webhooks/42/tok")
                .query_param("wait", "true")
                .json_body_includes(r#"{"flags": 32832}"#);
            then.status(200).json_body(fixtures::message(13, 5));
        })
        .await;
    let client = client(&server.base_url());
    let i = InteractionRef {
        id: 1,
        token: InteractionToken::new("tok"),
    };
    assert!(client.defer(&i, true).await.is_done());
    assert!(client.defer_update(&i).await.is_done());
    let suggestion = AutocompleteChoice {
        name: "Spam".into(),
        value: "u".into(),
    };
    assert!(client.autocomplete(&i, &[suggestion]).await.is_done());
    let void = quack_text::case::void_modal(
        quack_core::ids::CaseId::from_uuid(uuid::Uuid::from_u128(1)),
        Some(4),
    );
    assert!(
        client
            .open_modal(&i, &crate::render::render_modal(&void))
            .await
            .is_done()
    );
    assert!(
        client
            .edit_interaction_original(&i.token, &rendered())
            .await
            .is_done()
    );
    let sent = client
        .followup(&i.token, &rendered(), true)
        .await
        .done()
        .unwrap();
    assert_eq!(sent.message_id, MessageId::new(13).unwrap());
    for mock in [defer, update, choices, modal, original, followup] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn fetching_messages_converts_them() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/5/messages/10");
            then.status(200).json_body(fixtures::message(10, 5));
        })
        .await;
    let history = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/channels/5/messages")
                .query_param("limit", "100")
                .query_param("before", "10");
            then.status(200)
                .json_body(json!([fixtures::message(9, 5), fixtures::message(8, 5)]));
        })
        .await;
    let client = client(&server.base_url());
    let message = client
        .fetch_message(c(5), MessageId::new(10).unwrap())
        .await
        .done()
        .unwrap();
    assert_eq!(message.content, "hello");
    assert_eq!(message.author.id, AccountId::new(9).unwrap());
    assert_eq!(
        message.attachments[0].content_type.as_deref(),
        Some("image/png")
    );
    assert_eq!(message.embeds[0].kind, "rich");
    assert_eq!(message.guild_id, None);
    let page = client
        .fetch_history(c(5), Some(MessageId::new(10).unwrap()), 200)
        .await
        .done()
        .unwrap();
    assert_eq!(page.len(), 2);
    history.assert_async().await;
}
