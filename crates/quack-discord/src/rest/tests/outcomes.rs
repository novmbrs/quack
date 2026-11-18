//! How every kind of answer, or its absence, is classified.

use std::time::{Duration, Instant};

use httpmock::MockServer;
use quack_core::ids::{GuildId, MemberId};
use serde_json::json;

use super::{client, closed_port, raw_server};
use crate::rest::{NotSentCause, Outcome, RefusalKind, UnknownCause};

fn g() -> GuildId {
    GuildId::new(1).unwrap()
}
fn m() -> MemberId {
    MemberId::new(2).unwrap()
}

#[tokio::test]
async fn every_mapped_error_code_is_a_typed_refusal() {
    let cases = [
        (403, 50013, RefusalKind::MissingPermissions),
        (403, 50001, RefusalKind::MissingAccess),
        (403, 50007, RefusalKind::CannotDm),
        (400, 50278, RefusalKind::NoMutualGuilds),
        (404, 10004, RefusalKind::UnknownGuild),
        (404, 10007, RefusalKind::UnknownMember),
        (404, 10013, RefusalKind::UnknownUser),
        (404, 10026, RefusalKind::UnknownBan),
        (404, 10008, RefusalKind::UnknownMessage),
        (404, 10003, RefusalKind::UnknownChannel),
        (413, 40005, RefusalKind::TooLarge),
        (400, 50035, RefusalKind::Other),
    ];
    let server = MockServer::start_async().await;
    let client = client(&server.base_url());
    for (status, code, kind) in cases {
        let mock = server
            .mock_async(|when, then| {
                when.method("DELETE").path("/api/v10/guilds/1/members/2");
                then.status(status)
                    .json_body(json!({"code": code, "message": "nope"}));
            })
            .await;
        match client.kick(g(), m(), "r").await {
            Outcome::Refused(r) => {
                assert_eq!((r.status, r.code, r.kind), (status, Some(code), kind));
            }
            other => panic!("{status} {code}: {other:?}"),
        }
        mock.delete_async().await;
    }
}

#[tokio::test]
async fn server_errors_are_unknown_and_never_retried() {
    let server = MockServer::start_async().await;
    let client = client(&server.base_url());
    for status in [500, 502, 503, 504] {
        let mock = server
            .mock_async(|when, then| {
                when.method("DELETE").path("/api/v10/guilds/1/members/2");
                then.status(status).body("<html>bad gateway</html>");
            })
            .await;
        assert_eq!(
            client.kick(g(), m(), "r").await,
            Outcome::Unknown(UnknownCause::ServerError(status))
        );
        assert_eq!(
            mock.calls_async().await,
            1,
            "a {status} must not be retried"
        );
        mock.delete_async().await;
    }
}

#[tokio::test]
async fn too_many_requests_surfaces_retry_after_without_retrying() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method("DELETE").path("/api/v10/guilds/1/members/2");
            then.status(429)
                .header("retry-after", "3")
                .header("x-ratelimit-scope", "user")
                .json_body(json!({"message": "You are being rate limited.", "retry_after": 2.5, "global": false}));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client.kick(g(), m(), "r").await,
        Outcome::RetryAfter(Duration::from_millis(2500))
    );
    assert_eq!(mock.calls_async().await, 1);
}

#[tokio::test]
async fn a_bucket_exhausted_beyond_the_budget_answers_retry_after_at_once() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/2");
            then.status(404)
                .header("x-ratelimit-bucket", "bans")
                .header("x-ratelimit-limit", "1")
                .header("x-ratelimit-remaining", "0")
                .header("x-ratelimit-reset-after", "60")
                .json_body(json!({"code": 10026}));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(client.ban(g(), m()).await, Outcome::Done(None));
    let started = Instant::now();
    match client.ban(g(), m()).await {
        Outcome::RetryAfter(d) => assert!(d > Duration::from_secs(50), "{d:?}"),
        other => panic!("expected RetryAfter, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(
        mock.calls_async().await,
        1,
        "nothing was sent while rate limited"
    );
}

#[tokio::test]
async fn a_bucket_that_resets_within_the_budget_waits() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method("GET").path("/api/v10/guilds/1/bans/2");
            then.status(404)
                .header("x-ratelimit-bucket", "bans")
                .header("x-ratelimit-limit", "1")
                .header("x-ratelimit-remaining", "0")
                .header("x-ratelimit-reset-after", "0.2")
                .json_body(json!({"code": 10026}));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(client.ban(g(), m()).await, Outcome::Done(None));
    let started = Instant::now();
    assert_eq!(client.ban(g(), m()).await, Outcome::Done(None));
    assert!(started.elapsed() >= Duration::from_millis(150));
    assert_eq!(mock.calls_async().await, 2);
}

#[tokio::test]
async fn a_slow_answer_is_unknown_after_the_deadline() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("PUT").path("/api/v10/guilds/1/bans/2");
            then.status(204).delay(Duration::from_secs(2));
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client.ban_member(g(), m(), 0, "r").await,
        Outcome::Unknown(UnknownCause::Timeout)
    );
}

#[tokio::test]
async fn a_refused_connection_was_never_sent() {
    let client = client(&closed_port().await);
    assert_eq!(
        client.ban_member(g(), m(), 0, "r").await,
        Outcome::NotSent(NotSentCause::Connection)
    );
}

#[tokio::test]
async fn a_connection_dropped_after_sending_is_unknown() {
    let client = client(&raw_server(b"").await);
    assert_eq!(
        client.ban_member(g(), m(), 0, "r").await,
        Outcome::Unknown(UnknownCause::Connection)
    );
}

#[tokio::test]
async fn a_success_whose_body_is_cut_off_is_unknown() {
    let response =
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 500\r\n\r\n{\"id\":";
    let client = client(&raw_server(response).await);
    assert_eq!(
        client
            .open_dm(quack_core::ids::AccountId::new(9).unwrap())
            .await,
        Outcome::Unknown(UnknownCause::Connection)
    );
}

#[tokio::test]
async fn a_success_quack_cannot_read_is_unknown() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("POST").path("/api/v10/users/@me/channels");
            then.status(200).body("{\"unexpected\": true}");
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client
            .open_dm(quack_core::ids::AccountId::new(9).unwrap())
            .await,
        Outcome::Unknown(UnknownCause::Malformed)
    );
}

#[tokio::test]
async fn a_closed_client_sends_nothing() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.any_request();
            then.status(204);
        })
        .await;
    let client = client(&server.base_url());
    client.close();
    assert_eq!(
        client.kick(g(), m(), "r").await,
        Outcome::NotSent(NotSentCause::ShuttingDown)
    );
    assert_eq!(
        client.download_attachment(&server.url("/a.png"), 10).await,
        Outcome::NotSent(NotSentCause::ShuttingDown)
    );
    assert_eq!(mock.calls_async().await, 0);
}

#[tokio::test]
async fn downloads_stay_on_allowed_hosts_and_under_the_limit() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/small.png");
            then.status(200).body("12345");
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/large.png");
            then.status(200).body("x".repeat(100));
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/gone.png");
            then.status(404);
        })
        .await;
    let elsewhere = format!("http://localhost:{}/small.png", server.address().port());
    server
        .mock_async(|when, then| {
            when.method("GET").path("/moved.png");
            then.status(302).header("location", elsewhere.as_str());
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method("GET").path("/hop.png");
            then.status(302).header("location", "/small.png");
        })
        .await;
    let client = client(&server.base_url());
    assert_eq!(
        client
            .download_attachment(&server.url("/small.png"), 10)
            .await,
        Outcome::Done(b"12345".to_vec())
    );
    assert_eq!(
        client
            .download_attachment(&server.url("/hop.png"), 10)
            .await,
        Outcome::Done(b"12345".to_vec())
    );
    assert_eq!(
        client
            .download_attachment(&server.url("/large.png"), 10)
            .await
            .refusal(),
        Some(RefusalKind::TooLarge)
    );
    assert_eq!(
        client
            .download_attachment(&server.url("/gone.png"), 10)
            .await
            .refusal(),
        Some(RefusalKind::Other)
    );
    assert_eq!(
        client
            .download_attachment(&server.url("/moved.png"), 10)
            .await,
        Outcome::NotSent(NotSentCause::Invalid)
    );
    assert_eq!(
        client
            .download_attachment("https://example.com/a.png", 10)
            .await,
        Outcome::NotSent(NotSentCause::Invalid)
    );
}

#[tokio::test]
async fn a_chunked_download_stops_at_the_limit() {
    let response = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n8\r\n12345678\r\n8\r\n12345678\r\n0\r\n\r\n";
    let base = raw_server(response).await;
    let client = client(&base);
    assert_eq!(
        client
            .download_attachment(&format!("{base}/a.png"), 10)
            .await
            .refusal(),
        Some(RefusalKind::TooLarge)
    );
    assert_eq!(
        client
            .download_attachment(&format!("{base}/a.png"), 16)
            .await,
        Outcome::Done(b"1234567812345678".to_vec())
    );
}
