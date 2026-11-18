//! Graceful shutdown with jobs in flight, through the same runner and
//! module shutdown `serve` uses: a job whose Discord call answers during
//! the drain finishes; one still waiting at the deadline is dropped
//! unsettled, and after a restart its DM is confirmed from the DM
//! channel's history instead of being sent twice.

use std::time::Duration;

use anyhow::{Context as _, Result};
use axum::http::{Method, StatusCode};
use quack_db::jobs::RunnerConfig;
use quack_testkit::fake::Call;
use quack_testkit::{ADMIN, MEMBER};
use serde_json::json;

use crate::common::{System, case_add, ix, moderator};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_drains_what_it_can_and_a_restart_resolves_the_rest() -> Result<()> {
    let mut sys = System::boot().await?;
    sys.install().await?;
    let admin = sys.login(ADMIN).await?;
    let created = sys
        .http(
            Method::POST,
            "/api/guilds/1001/rules",
            Some(&admin),
            Some(json!({
                "name": "Spam", "description": "", "reason": "Please don't spam.",
                "appealable": true, "counting_window_days": null, "fields": [],
                "levels": [{"from_case_count": 1, "action": null, "send_dm": true}]
            })),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let rule = created.json["id"].as_str().context("rule id")?.to_owned();
    let runner = |sys: &System| {
        let r = quack_engine::jobs::runner(
            sys.app.engine.clone(),
            RunnerConfig::new(4),
            sys.app.module_jobs(),
        );
        r.start();
        r
    };

    // 1. Discord answers during the drain: the DM job finishes.
    let gate = sys.fake.hold(Call::OpenDm);
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    let first = runner(&sys);
    tokio::time::timeout(Duration::from_secs(10), gate.reached()).await?;
    let release = {
        let gate = gate.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            gate.release();
        })
    };
    let drain = std::time::Instant::now();
    tokio::join!(
        first.shutdown(Duration::from_secs(5)),
        sys.app.modules.shutdown(Duration::from_secs(5))
    );
    release.await?;
    assert!(drain.elapsed() < Duration::from_secs(5), "drained early");
    assert_eq!(
        sys.db
            .count("SELECT count(*) FROM dms WHERE state = 'sent'")
            .await?,
        1,
        "the in-flight DM finished during the drain"
    );

    // 2. Discord accepts the DM but its answer is still on the way at the
    //    deadline: the job is dropped unsettled.
    let stuck = sys.fake.hold_after(Call::Send);
    sys.send(&ix(
        2,
        moderator(),
        sys.channel,
        &case_add(MEMBER, &rule, None),
    ))
    .await;
    let second = runner(&sys);
    tokio::time::timeout(Duration::from_secs(10), stuck.reached()).await?;
    let drain = std::time::Instant::now();
    second.shutdown(Duration::from_millis(500)).await;
    assert!(
        drain.elapsed() < Duration::from_secs(5),
        "shutdown kept its deadline"
    );
    assert_eq!(sys.dms(MEMBER).len(), 2, "Discord has the second DM");

    // The next process: the lease has expired; the DM job finds the
    // attempt that may have reached Discord and doesn't send it again.
    sys.restart().await?;
    quack_testkit::expire_leases_with(&sys.app.engine).await?;
    sys.settle().await?;
    stuck.release();
    // The attempt was recorded before the call; the re-run finds the
    // message in the DM channel by its nonce and records it as sent.
    assert_eq!(
        sys.db
            .count("SELECT count(*) FROM dms WHERE state = 'sent'")
            .await?,
        2,
        "confirmed from the DM channel, not sent twice"
    );
    assert_eq!(sys.dms(MEMBER).len(), 2);
    sys.fake.clear_scripts();
    sys.assert_no_duplicate_effects();
    Ok(())
}
