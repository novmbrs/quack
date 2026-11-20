//! Ops in Discord (§21): with the standard server as the ops server and
//! the administrator as the operator, installs are posted to
//! `#ops-installs`, `/ops` answers only the operator, and **Requeue** on a
//! dead-job alert edits the alert.

use anyhow::Result;
use quack_testkit::{ADMIN, GUILD};
use quack_text::render::plain_msg;
use serde_json::json;

use crate::common::{Sent, System, admin, click, ix, moderator};

fn ops_command(sub: &str) -> serde_json::Value {
    json!({"id": "102", "name": "ops", "type": 1, "options": [{"name": sub, "type": 1}]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub(crate) async fn ops_posts_installs_and_answers_only_operators() -> Result<()> {
    let sys = System::boot_with(&[
        ("ops_guild_id", GUILD.to_string()),
        ("ops_user_ids", ADMIN.to_string()),
    ])
    .await?;
    let ops = sys.app.ops.clone().expect("ops is configured");
    let channels = ops.prepare_channels().await.expect("the channels");

    // Joining a server posts to #ops-installs.
    sys.install().await?;
    sys.settle().await?;
    let installs = sys.posted(channels.installs.unwrap());
    assert_eq!(installs.len(), 1, "{installs:#?}");
    assert!(
        installs[0].contains("Quack joined a server"),
        "{}",
        installs[0]
    );

    // The operator gets the board; a moderator gets a refusal.
    let sent = sys
        .send(&ix(2, admin(), sys.channel, &ops_command("status")))
        .await;
    assert!(
        matches!(sent[0], Sent::Defer { ephemeral: true }),
        "{sent:?}"
    );
    let board = plain_msg(&sys.answer());
    assert!(board.contains("Quack status"), "{board}");
    sys.send(&ix(2, moderator(), sys.channel, &ops_command("status")))
        .await;
    let refused = plain_msg(&sys.answer());
    assert!(refused.contains("Only Quack's operators"), "{refused}");

    // A dead job is posted with Requeue; the click edits the alert.
    sys.db
        .execute(&format!(
            "INSERT INTO jobs (kind, key, guild_id, payload, max_attempts, attempts, dead_at, last_error)
             VALUES ('send_dm', 'send_dm:ops-test', {GUILD}, '{{}}', 3, 3, now(), 'discord_unavailable')"
        ))
        .await?;
    ops.check_alerts().await?;
    sys.settle().await?;
    let alerts = channels.alerts.unwrap();
    let alert = sys.fake.world(|w| {
        w.sent_to(alerts)
            .last()
            .map(|m| (m.fetched.id, m.msg.clone()))
    });
    let (alert_id, alert_msg) = alert.expect("the alert");
    let requeue = alert_msg
        .unwrap()
        .rows
        .iter()
        .find_map(|row| match row {
            quack_text::Row::Buttons(b) => b.iter().find(|b| b.label == "Requeue").cloned(),
            quack_text::Row::Select(_) => None,
        })
        .expect("a Requeue button");
    let quack_text::model::ButtonTarget::Action(action) = requeue.target else {
        panic!("an action button");
    };
    let id = quack_discord::custom_id::CustomId::from(&action).encode();
    let mut payload = click(admin(), alerts, &id, false);
    payload["message"]["id"] = json!(alert_id.to_string());
    sys.send(&payload).await;
    assert!(plain_msg(&sys.answer()).contains("requeued it"));
    let edited = sys.posted(alerts);
    assert!(
        edited.last().unwrap().contains("requeued it"),
        "{edited:#?}"
    );
    assert_eq!(
        sys.db
            .count(
                "SELECT count(*) FROM jobs WHERE key = 'send_dm:ops-test' AND dead_at IS NOT NULL"
            )
            .await?,
        0
    );
    Ok(())
}
