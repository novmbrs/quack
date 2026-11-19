use std::sync::Arc;

use httpmock::prelude::*;
use quack_core::ids::{ChannelId, GuildId, MemberId, MessageId};
use quack_discord::cache::{GuildCache, MemberCache};
use quack_discord::facts::Facts;
use quack_discord::{ClientConfig, DiscordClient, RefusalKind};
use quack_engine::Discord;
use quack_engine::port as engine;
use quack_text::{Msg, Text, Tone};

use super::DiscordPort;
use super::convert;

#[test]
fn nonces_convert() {
    assert!(convert::nonce(&engine::Nonce::new("case-dm-123")).is_some());
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
