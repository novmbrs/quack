//! Snapshots of the card images' words and numbers (the pictures
//! themselves are drawn and checked in `quack-card`).

use std::fmt::Write as _;

use super::samples::ops::{overview, status};
use crate::card::{Card, Section};
use crate::ops::{self, Backlog, Status};

/// The card as lines of text, top to bottom.
fn plain(c: &Card) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "tone: {:?} · badge: {:?}", c.tone, c.badge);
    let _ = writeln!(out, "# {}", c.title);
    if let Some(sub) = &c.subtitle {
        let _ = writeln!(out, "  {sub}");
    }
    for s in &c.sections {
        match s {
            Section::Tiles(tiles) => {
                for t in tiles {
                    let _ = writeln!(
                        out,
                        "[{} {:?}] {} — {}",
                        t.label,
                        t.tone,
                        t.value,
                        t.note.as_deref().unwrap_or("")
                    );
                }
            }
            Section::Checks { title, rows } => {
                let _ = writeln!(out, "== {}", title.as_deref().unwrap_or("(checks)"));
                for r in rows {
                    let _ = writeln!(
                        out,
                        "• {} ({}) → {} [{:?}]",
                        r.label,
                        r.detail.as_deref().unwrap_or(""),
                        r.state,
                        r.tone
                    );
                }
            }
            Section::Columns(col) => {
                let _ = writeln!(
                    out,
                    "== {} | {} | {:?} failed {:?} | {} → {}",
                    col.title,
                    col.note.as_deref().unwrap_or(""),
                    col.values,
                    col.failed,
                    col.start,
                    col.end
                );
            }
            Section::Bars {
                title,
                rows,
                out_of,
            } => {
                let _ = writeln!(out, "== {title} (out of {out_of:?})");
                for b in rows {
                    let _ = writeln!(out, "{} {} {}", b.label, b.value, b.shown);
                }
            }
            Section::Table {
                title,
                header,
                rows,
            } => {
                let _ = writeln!(out, "== {title}\n{}", header.join(" | "));
                for r in rows {
                    let _ = writeln!(out, "{}", r.join(" | "));
                }
            }
        }
    }
    let _ = writeln!(
        out,
        "-- {} | {}",
        c.footer,
        c.footer_right.as_deref().unwrap_or("")
    );
    let _ = writeln!(out, "alt: {}", c.alt);
    out
}

fn troubled() -> Status {
    Status {
        backlog: vec![Backlog {
            kind: "send_dm",
            ready: 40,
            oldest_seconds: 720,
        }],
        dead_jobs: 2,
        pool_used: 20,
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

#[test]
fn cards() {
    let cards = [
        ("ops healthy", ops::status_card(&status())),
        ("ops needs a look", ops::status_card(&troubled())),
        ("ops down", ops::status_card(&down())),
        ("public operational", ops::public_card(&status())),
        ("public degraded", ops::public_card(&troubled())),
        ("public outage", ops::public_card(&down())),
        (
            "metrics overview",
            ops::metrics_overview_card(&overview(1_100)),
        ),
    ];
    let mut out = String::new();
    for (name, card) in &cards {
        let _ = writeln!(out, "## {name}\n{}", plain(card));
    }
    insta::assert_snapshot!("cards", out);
}
