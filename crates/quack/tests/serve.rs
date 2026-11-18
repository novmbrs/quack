//! `quack serve` end to end against real PostgreSQL 18, with Discord
//! turned off through the internal test seam: readiness flips to ready,
//! a job enqueued through the database completes on the runner, and a
//! shutdown signal stops everything cleanly within the deadline.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use quack::config::{Need, validate};
use quack::serve::{DiscordMode, Options, Readiness, SHUTDOWN_DEADLINE, run};
use quack_core::ids::{GuildId, MessageRowId};
use quack_db::jobs::{NewJob, enqueue};
use quack_testkit::db::TestDb;

/// The test database's URL: the server's URL with the test database's name.
fn url_of(test: &TestDb) -> String {
    let base = quack_testkit::db::server_url().unwrap();
    let mut url = reqwest::Url::parse(&base).unwrap();
    let name = test.db.connect_options().get_database().unwrap().to_owned();
    url.set_path(&format!("/{name}"));
    url.to_string()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn status(client: &reqwest::Client, url: &str) -> Option<u16> {
    client
        .get(url)
        .send()
        .await
        .ok()
        .map(|r| r.status().as_u16())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serve_becomes_ready_runs_a_job_and_shuts_down_cleanly() {
    let test = TestDb::new().await.unwrap();
    let dashboard = std::env::temp_dir().join(format!("quack-serve-test-{}", std::process::id()));
    std::fs::create_dir_all(&dashboard).unwrap();
    std::fs::write(dashboard.join("index.html"), "<html></html>").unwrap();
    let port = free_port();
    let vars = [
        ("database_url", url_of(&test)),
        ("discord_token", "token".to_owned()),
        ("discord_application_id", "42".to_owned()),
        ("discord_client_secret", "secret".to_owned()),
        ("public_url", "http://localhost:5173".to_owned()),
        (
            "session_secret",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        ),
        ("dashboard_dir", dashboard.to_str().unwrap().to_owned()),
        ("listen_addr", format!("127.0.0.1:{port}")),
        ("database_max_connections", "4".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let config = validate(Need::Serve, &vars).unwrap();
    let options = Options {
        migrate: true,
        // Nothing listens on port 9: any Discord call would fail fast.
        discord: DiscordMode::Off {
            api_base: "http://127.0.0.1:9".to_owned(),
        },
    };
    let ready = Arc::new(Readiness::default());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serve = Box::pin(run(&config, &options, Arc::clone(&ready), async move {
        let _ = stopped.await;
    }));
    // `serve` runs on this task next to the checks; the checks send the
    // shutdown signal at the end and time how long the exit takes.
    let checks = checks(&test, &ready, port, stop);
    let (result, signalled) = tokio::time::timeout(Duration::from_secs(90), async {
        tokio::join!(serve, checks)
    })
    .await
    .expect("serve and the checks finished");
    assert!(result.is_ok(), "{result:?}");
    assert!(
        signalled.elapsed() < SHUTDOWN_DEADLINE,
        "took {:?}",
        signalled.elapsed()
    );
    assert!(!ready.is_ready(), "not ready after shutdown");
    let http = reqwest::Client::new();
    assert_eq!(
        status(&http, &format!("http://127.0.0.1:{port}/livez")).await,
        None,
        "HTTP stopped"
    );
    let fence = test
        .count("SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND database = (SELECT oid FROM pg_database WHERE datname = current_database())")
        .await
        .unwrap();
    assert_eq!(fence, 0, "the fence was released");
}

/// Waits for readiness, runs a job, then signals shutdown; returns when
/// the signal was sent.
async fn checks(
    test: &TestDb,
    ready: &Readiness,
    port: u16,
    stop: tokio::sync::oneshot::Sender<()>,
) -> Instant {
    // Readiness flips once the fence, the workers, and the database pass.
    let http = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let started = Instant::now();
    loop {
        if status(&http, &format!("{base}/readyz")).await == Some(200) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "never became ready"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready.is_ready());
    assert_eq!(status(&http, &format!("{base}/livez")).await, Some(200));

    // A job enqueued in a transaction completes on the runner.
    let guild: GuildId = quack_testkit::sf(1001);
    let mut tx = test.db.begin().await.unwrap();
    quack_db::guilds::upsert(
        &mut tx,
        &quack_db::guilds::GuildFacts {
            id: guild,
            name: "Server".to_owned(),
            icon_hash: None,
            owner_id: quack_testkit::sf(10),
            mfa_required: false,
        },
    )
    .await
    .unwrap();
    quack_db::guilds::ensure_children(&mut tx, guild)
        .await
        .unwrap();
    let job = enqueue(
        &mut tx,
        &NewJob::sync_message(guild, MessageRowId::from_uuid(uuid::Uuid::now_v7())),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let waited = Instant::now();
    loop {
        let left = test
            .count(&format!(
                "SELECT count(*) FROM jobs WHERE id = '{}' AND dead_at IS NULL",
                job.get()
            ))
            .await
            .unwrap();
        if left == 0 {
            break;
        }
        assert!(
            waited.elapsed() < Duration::from_secs(15),
            "the job never completed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let housekeeping = test
        .count("SELECT count(*) FROM jobs WHERE kind = 'housekeeping'")
        .await
        .unwrap();
    assert_eq!(housekeeping, 1, "housekeeping is scheduled");

    // The shutdown signal.
    let signalled = Instant::now();
    stop.send(()).unwrap();
    signalled
}
