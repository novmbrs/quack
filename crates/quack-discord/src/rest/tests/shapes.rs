//! What each call sends: method, path, headers, and JSON body.

use httpmock::MockServer;
use quack_core::ids::{AccountId, AttachmentId, ChannelId, GuildId, MemberId, MessageId, RoleId};
use quack_core::permissions::{Overwrite, OverwriteTarget, Permissions};
use quack_text::{Msg, Text, Tone};
use serde_json::json;
use time::macros::datetime;

use super::fixtures;
use super::client;
use crate::model::{ChannelSpec, FileEdit, InteractionRef, InteractionToken, Nonce, OutgoingFile};
use crate::render::{RenderedMessage, render};
use crate::rest::{AutocompleteChoice, MessageBody, Outcome};

fn g(id: u64) -> GuildId {
    GuildId::new(id).unwrap()
}
fn m(id: u64) -> MemberId {
    MemberId::new(id).unwrap()
}
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
async fn moderation_actions_send_the_audit_log_reason() {
    let server = MockServer::start_async().await;
    let reason = "Quack%20case%20%234%3A%20Spam%20%26%20scams";
    let kick = server
        .mock_async(|when, then| {
            when.method("DELETE")
                .path("/api/v10/guilds/1/members/2")
                .header("authorization", "Bot TOKEN")
                .header("x-audit-log-reason", reason)
                .header_prefix("user-agent", "DiscordBot (");
            then.status(204);
        })
        .await;
    let timeout = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/guilds/1/members/3")
                .header("x-audit-log-reason", reason)
                .json_body(json!({"communication_disabled_until": "2026-11-17T12:00:00Z"}));
            then.status(200).json_body(fixtures::member(3, &[]));
        })
        .await;
    let untimeout = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/guilds/1/members/4")
                .json_body(json!({"communication_disabled_until": null}));
            then.status(200).json_body(fixtures::member(4, &[]));
        })
        .await;
    let ban = server
        .mock_async(|when, then| {
            when.method("PUT")
                .path("/api/v10/guilds/1/bans/5")
                .header("x-audit-log-reason", reason)
                .json_body(json!({"delete_message_seconds": 604_800}));
            then.status(204);
        })
        .await;
    let unban = server
        .mock_async(|when, then| {
            when.method("DELETE")
                .path("/api/v10/guilds/1/bans/6")
                .header("x-audit-log-reason", "Quack%20case%20%234%20voided");
            then.status(204);
        })
        .await;
    let client = client(&server.base_url());
    let why = "Quack case #4: Spam & scams";
    assert_eq!(client.kick(g(1), m(2), why).await, Outcome::Done(()));
    let until = datetime!(2026-11-17 12:00 UTC);
    assert_eq!(client.timeout(g(1), m(3), until, why).await, Outcome::Done(()));
    assert_eq!(client.remove_timeout(g(1), m(4), why).await, Outcome::Done(()));
    assert_eq!(client.ban_member(g(1), m(5), 999_999, why).await, Outcome::Done(()));
    assert_eq!(
        client.unban(g(1), m(6), "Quack case #4 voided").await,
        Outcome::Done(())
    );
    for mock in [kick, timeout, untimeout, ban, unban] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn reads_answer_absent_only_for_their_unknown_code() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/2");
            then.status(200).json_body(fixtures::member(2, &[7]));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/3");
            then.status(404).json_body(json!({"code": 10007, "message": "Unknown Member"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/4");
            then.status(404).json_body(json!({"code": 10013, "message": "Unknown User"}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/2");
            then.status(200)
                .json_body(json!({"reason": "Quack case #3: Spam", "user": fixtures::user(2, "x")}));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/3");
            then.status(404).json_body(json!({"code": 10026, "message": "Unknown Ban"}));
        })
        .await;
    let client = client(&server.base_url());
    let Outcome::Done(Some(member)) = client.member(g(1), m(2)).await else {
        panic!("expected a member");
    };
    assert_eq!(member.role_ids, vec![RoleId::new(7).unwrap()]);
    assert_eq!(member.joined_at, Some(datetime!(2024-01-02 03:04:05 UTC)));
    assert_eq!(client.member(g(1), m(3)).await, Outcome::Done(None));
    assert_eq!(
        client.member(g(1), m(4)).await.refusal(),
        Some(crate::RefusalKind::UnknownUser)
    );
    let ban = client.get_ban(g(1), m(2)).await.done().flatten().unwrap();
    assert_eq!(ban.case_marker(), Some(3));
    assert_eq!(client.get_ban(g(1), m(3)).await, Outcome::Done(None));
}

#[tokio::test]
async fn guild_facts_compute_quacks_permissions() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1");
            then.status(200).json_body(fixtures::guild(
                1,
                8,
                vec![fixtures::role(1, 0, 1024), fixtures::role(20, 5, 4)],
            ));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/members/50");
            then.status(200).json_body(fixtures::member(50, &[20]));
        })
        .await;
    let client = client(&server.base_url());
    let now = datetime!(2026-11-17 0:00 UTC);
    let facts = client
        .guild_facts(g(1), AccountId::new(50).unwrap(), now)
        .await
        .done()
        .unwrap();
    assert!(facts.mfa_required);
    assert_eq!(facts.owner_id, AccountId::new(8).unwrap());
    let quack = facts.quack.unwrap();
    assert_eq!(quack.highest_role_position, 5);
    assert_eq!(quack.permissions, Permissions(1024 | 4));
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
        .send(c(5), &MessageBody::rendered(message.clone()), &[], Some(&nonce))
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
            when.method("POST")
                .path(path)
                .json_body(json!({"type": 8, "data": {"choices": [{"name": "Spam", "value": "u"}]}}));
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
    assert!(client.open_modal(&i, &crate::render::render_modal(&void)).await.is_done());
    assert!(client.edit_original(&i.token, &rendered()).await.is_done());
    let sent = client.followup(&i.token, &rendered(), true).await.done().unwrap();
    assert_eq!(sent.message_id, MessageId::new(13).unwrap());
    for mock in [defer, update, choices, modal, original, followup] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn channels_threads_and_dms() {
    let server = MockServer::start_async().await;
    let create = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/guilds/1/channels")
                .json_body(json!({
                    "name": "moderation-log",
                    "type": 0,
                    "topic": "Quack posts here.",
                    "permission_overwrites": [
                        {"id": "1", "type": 0, "allow": "0", "deny": "1024"},
                        {"id": "50", "type": 1, "allow": "117760", "deny": "0"}
                    ]
                }));
            then.status(201).json_body(fixtures::channel(30, 1, 0));
        })
        .await;
    let thread = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/channels/30/threads")
                .json_body(json!({
                    "name": "ticket-abc",
                    "type": 12,
                    "invitable": false,
                    "auto_archive_duration": 1440
                }));
            then.status(201).json_body(fixtures::channel(31, 1, 12));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/31/thread-members/7");
            then.status(404).json_body(json!({"code": 10007, "message": "Unknown Member"}));
        })
        .await;
    let freeze = server
        .mock_async(|when, then| {
            when.method("PATCH")
                .path("/api/v10/channels/31")
                .json_body(json!({"archived": true, "locked": true}));
            then.status(200).json_body(fixtures::channel(31, 1, 12));
        })
        .await;
    let dm = server
        .mock_async(|when, then| {
            when.method("POST")
                .path("/api/v10/users/@me/channels")
                .json_body(json!({"recipient_id": "9"}));
            then.status(200).json_body(json!({"id": "40", "type": 1, "recipients": [fixtures::user(9, "x")]}));
        })
        .await;
    let search = server
        .mock_async(|when, then| {
            when.method("GET")
                .path("/api/v10/guilds/1/members/search")
                .query_param("query", "a b")
                .query_param("limit", "5");
            then.status(200).json_body(json!([fixtures::member(2, &[])]));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/channels/30");
            then.status(200).json_body(fixtures::channel(30, 1, 0));
        })
        .await;
    let client = client(&server.base_url());
    let spec = ChannelSpec {
        name: "moderation-log".into(),
        topic: Some("Quack posts here.".into()),
        parent_id: None,
        overwrites: vec![
            Overwrite {
                target: OverwriteTarget::Role(RoleId::new(1).unwrap()),
                allow: Permissions::NONE,
                deny: Permissions(1024),
            },
            Overwrite {
                target: OverwriteTarget::Member(m(50)),
                allow: Permissions(1024 | 2048 | 65536 | 16384 | 32768),
                deny: Permissions::NONE,
            },
        ],
    };
    assert_eq!(client.create_channel(g(1), &spec).await, Outcome::Done(c(30)));
    assert_eq!(
        client.create_private_thread(c(30), "ticket-abc").await,
        Outcome::Done(c(31))
    );
    assert_eq!(
        client.is_thread_member(c(31), AccountId::new(7).unwrap()).await,
        Outcome::Done(false)
    );
    assert!(client.set_thread_state(c(31), true, true).await.is_done());
    assert_eq!(
        client.open_dm(AccountId::new(9).unwrap()).await,
        Outcome::Done(c(40))
    );
    assert_eq!(client.search_members(g(1), "a b", 5).await.done().map(|v| v.len()), Some(1));
    let fetched = client.channel(c(30)).await.done().unwrap();
    assert_eq!(fetched.overwrites.len(), 2);
    for mock in [create, thread, freeze, dm, search] {
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
    assert_eq!(message.attachments[0].content_type.as_deref(), Some("image/png"));
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
