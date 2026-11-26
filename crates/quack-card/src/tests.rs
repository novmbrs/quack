//! Every card draws, at the expected size. With `QUACK_CARD_PREVIEW` set
//! to a directory, the PNGs are written there to look at.

use quack_text::card::Card;
use quack_text::ops::{self, Backlog, Fleet, MetricsOverview, Status, Trend, Window};
use time::macros::datetime;

fn status() -> Status {
    let cases = vec![
        0, 1, 0, 0, 2, 1, 0, 0, 1, 3, 2, 1, 0, 1, 2, 4, 1, 0, 0, 1, 2, 1, 0, 1,
    ];
    let runs = (0..24u64)
        .map(|h| (8 + (h * 7) % 13, u64::from(h == 15)))
        .collect();
    Status {
        version: "5.3.0".to_owned(),
        commit: Some("a762256d1f".to_owned()),
        started_at: datetime!(2026-11-19 20:38 UTC),
        gateway_connected: true,
        latency_ms: Some(40),
        servers: 894,
        members: 288_412,
        trend: Trend {
            cases: cases.iter().sum(),
            appeals: 3,
            tickets: 6,
            joins: 1,
            leaves: 1,
            job_runs: 231,
            job_failures: 1,
            runs_seconds: 44_700,
            cases_hourly: cases,
            runs_hourly: runs,
        },
        backlog: Vec::new(),
        dead_jobs: 0,
        failed_actions: 0,
        open_appeals: 4,
        open_tickets: 18,
        pool_used: 1,
        pool_size: 20,
        updated_at: datetime!(2026-11-20 09:03 UTC),
    }
}

fn troubled() -> Status {
    Status {
        backlog: vec![Backlog {
            kind: "send_dm",
            ready: 40,
            oldest_seconds: 720,
        }],
        dead_jobs: 2,
        failed_actions: 3,
        ..status()
    }
}

fn down() -> Status {
    Status {
        gateway_connected: false,
        latency_ms: None,
        ..status()
    }
}

fn overview() -> MetricsOverview {
    let w = |day, week, month, all| Window {
        day,
        week,
        month,
        all,
    };
    MetricsOverview {
        fleet: Fleet {
            servers: 894,
            counted: 880,
            members: 288_412,
            median_members: Some(212),
            sizes: [412, 300, 140, 26, 2],
            tickets_on: 210,
            activity_log_on: 340,
            honeypot_on: 55,
        },
        cases: w(24, 180, 790, 36_020),
        voided: w(1, 6, 30, 1_230),
        appeals: w(3, 24, 100, 1_640),
        tickets: w(6, 50, 220, 6_970),
        actions_done: w(18, 130, 560, 25_400),
        actions_failed: w(0, 2, 9, 410),
        joins: w(1, 9, 40, 2_460),
        leaves: w(1, 4, 18, 1_566),
        daily_cases: vec![22, 30, 41, 25, 8, 12, 44, 51, 50, 39, 37, 45, 52, 24],
        updated_at: datetime!(2026-11-20 09:03 UTC),
    }
}

fn cards() -> Vec<(&'static str, Card)> {
    vec![
        ("ops-healthy", ops::status_card(&status())),
        ("ops-needs-a-look", ops::status_card(&troubled())),
        ("ops-down", ops::status_card(&down())),
        ("public-operational", ops::public_card(&status())),
        ("public-degraded", ops::public_card(&troubled())),
        ("public-outage", ops::public_card(&down())),
        ("metrics-overview", ops::metrics_overview_card(&overview())),
    ]
}

#[test]
fn every_card_draws() {
    let preview = std::env::var_os("QUACK_CARD_PREVIEW");
    for (name, card) in cards() {
        let png = crate::png(&card).unwrap();
        assert_eq!(&png[1..4], b"PNG", "{name}");
        // The IHDR width: twice the logical width.
        let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        assert_eq!(width, 1_760, "{name}");
        assert!(png.len() < 1_000_000, "{name} is {} bytes", png.len());
        if let Some(dir) = &preview {
            std::fs::write(std::path::Path::new(dir).join(format!("{name}.png")), &png).unwrap();
        }
    }
}

#[test]
fn text_is_escaped() {
    let mut card = ops::status_card(&status());
    card.title = "<b>&\"'".to_owned();
    let svg = crate::svg(&card);
    assert!(svg.contains("&lt;b&gt;&amp;&quot;&apos;"));
    assert!(crate::png(&card).is_ok());
}
