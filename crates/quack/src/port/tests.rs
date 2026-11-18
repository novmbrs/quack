use std::sync::Arc;
use std::time::Duration;

use httpmock::prelude::*;
use quack_core::ids::{ChannelId, GuildId, MemberId, MessageId};
use quack_discord::cache::{GuildCache, MemberCache};
use quack_discord::facts::Facts;
use quack_discord::{
    ClientConfig, DiscordClient, NotSentCause, Outcome, Refusal, RefusalKind, UnknownCause,
};
use quack_engine::Discord;
use quack_engine::port as engine;
use quack_text::{Msg, Text, Tone};

use super::DiscordPort;
use super::convert;

#[test]
fn outcomes_map_variant_for_variant() {
    assert_eq!(
        convert::outcome(Outcome::Done(3), |n| n + 1),
        engine::Outcome::Done(4)
    );
    let refused: engine::Outcome<()> =
        convert::outcome(Outcome::Refused(Refusal::new(403, Some(50_013))), |()| ());
    assert_eq!(
        refused,
        engine::Outcome::Refused(engine::Refusal::of(engine::RefusalKind::MissingPermissions))
    );
    assert_eq!(
        convert::outcome::<(), ()>(Outcome::RetryAfter(Duration::from_secs(2)), |()| ()),
        engine::Outcome::RetryAfter(Duration::from_secs(2))
    );
    assert_eq!(
        convert::unknown(UnknownCause::Malformed),
        engine::UnknownCause::Malformed
    );
    assert_eq!(
        convert::unknown(UnknownCause::ServerError(502)),
        engine::UnknownCause::ServerError(502)
    );
    assert_eq!(
        convert::not_sent(NotSentCause::Invalid),
        engine::NotSentCause::Invalid
    );
    assert_eq!(
        convert::not_sent(NotSentCause::ShuttingDown),
        engine::NotSentCause::ShuttingDown
    );
}

#[test]
fn every_refusal_kind_keeps_its_meaning() {
    for (status, code, kind) in [
        (403, Some(50_013), engine::RefusalKind::MissingPermissions),
        (403, Some(50_001), engine::RefusalKind::MissingAccess),
        (403, Some(50_007), engine::RefusalKind::CannotDm),
        (403, Some(50_278), engine::RefusalKind::NoMutualGuilds),
        (404, Some(10_004), engine::RefusalKind::UnknownGuild),
        (404, Some(10_007), engine::RefusalKind::UnknownMember),
        (404, Some(10_013), engine::RefusalKind::UnknownUser),
        (404, Some(10_026), engine::RefusalKind::UnknownBan),
        (404, Some(10_008), engine::RefusalKind::UnknownMessage),
        (404, Some(10_003), engine::RefusalKind::UnknownChannel),
        (413, Some(40_005), engine::RefusalKind::TooLarge),
        (400, None, engine::RefusalKind::Other),
    ] {
        let r = convert::refusal(Refusal::new(status, code));
        assert_eq!((r.status, r.code, r.kind), (status, code, kind));
    }
}

#[test]
fn messages_and_files_convert() {
    let file = engine::OutgoingFile {
        name: "a.png".into(),
        content_type: Some("image/png".into()),
        bytes: vec![1, 2],
    };
    let converted = convert::file(&file);
    assert_eq!(
        (converted.name.as_str(), converted.bytes.len()),
        ("a.png", 2)
    );
    assert!(matches!(
        convert::file_edit(&engine::FileEdit::Replace(file)),
        quack_discord::model::FileEdit::Replace(_)
    ));
    assert!(convert::nonce(&engine::Nonce::new("case-dm-123")).is_some());
    assert_eq!(
        convert::token(&engine::InteractionToken::new("t")).expose(),
        "t"
    );
    let fresh = convert::fresh(engine::Fresh::Rest);
    assert_eq!(fresh, quack_discord::facts::Fresh::Rest);
}

fn port(server: &MockServer) -> DiscordPort {
    let mut config = ClientConfig::new("token", 42);
    config.api_base = server.base_url();
    let client = DiscordClient::new(&config).unwrap();
    let facts = Facts::new(
        client,
        Arc::new(GuildCache::new()),
        Arc::new(MemberCache::new(10, MemberCache::REST_TTL)),
    );
    DiscordPort::new(facts, quack_bot::CommandMentions::default())
}

#[tokio::test]
async fn send_renders_components_v2_with_the_nonce_and_maps_the_answer() {
    let server = MockServer::start_async().await;
    let created = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/channels/5/messages")
                .body_includes("\"enforce_nonce\":true")
                .body_includes("\"nonce\":\"dm-1\"")
                .body_includes("\"flags\":32768");
            then.status(200).json_body(serde_json::json!({
                "id": "77", "channel_id": "5", "author": {"id": "20", "username": "Quack", "discriminator": "0", "global_name": null, "avatar": null, "bot": true},
                "content": "", "timestamp": "2026-11-18T12:00:00.000000+00:00", "edited_timestamp": null,
                "tts": false, "mention_everyone": false, "mentions": [], "mention_roles": [], "attachments": [],
                "embeds": [], "pinned": false, "type": 0, "flags": 32768
            }));
        })
        .await;
    let port = port(&server);
    let msg = Msg::text(Tone::Neutral, Text::fixed("Hello"));
    let channel: ChannelId = ChannelId::new(5).unwrap();
    let sent = port
        .send(channel, &msg, &[], &engine::Nonce::new("dm-1"))
        .await;
    created.assert_async().await;
    assert_eq!(
        sent,
        engine::Outcome::Done(engine::SentMessage {
            channel_id: channel,
            message_id: MessageId::new(77).unwrap(),
            attachments: Vec::new()
        })
    );
}

#[tokio::test]
async fn a_kick_refused_with_missing_permissions_is_a_refusal() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(DELETE).path("/guilds/1/members/9");
            then.status(403)
                .json_body(serde_json::json!({"code": 50013, "message": "Missing Permissions"}));
        })
        .await;
    let port = port(&server);
    let out = port
        .kick(
            GuildId::new(1).unwrap(),
            MemberId::new(9).unwrap(),
            "case #1",
        )
        .await;
    assert_eq!(
        out,
        engine::Outcome::Refused(engine::Refusal::of(engine::RefusalKind::MissingPermissions))
    );
    let _ = RefusalKind::Other;
}
