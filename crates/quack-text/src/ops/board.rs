//! The status boards (§21.6): `ops.status`, the full board for operators,
//! and `ops.public_status`, the short one anyone may see. Both are an
//! image (a [`Card`]) with a line or two of text under it.

use time::{OffsetDateTime, UtcOffset};

use crate::actions::ButtonAction;
use crate::card::{Card, Check, Columns, Section, Tile};
use crate::fmt;
use crate::model::{Block, Button, ButtonStyle, Inline, Msg, Text, TimeStyle, Tone};

/// The ops board's image name.
pub const STATUS_IMAGE: &str = "quack-ops-status.png";
/// The public board's image name.
pub const PUBLIC_IMAGE: &str = "quack-status.png";

/// One job kind with work waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backlog {
    /// The kind's name.
    pub kind: &'static str,
    /// Jobs ready to run.
    pub ready: u64,
    /// The oldest ready job's wait, in seconds.
    pub oldest_seconds: u64,
}

/// What happened in the last 24 hours, across every server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trend {
    /// Cases created.
    pub cases: u64,
    /// Appeals sent.
    pub appeals: u64,
    /// Tickets opened.
    pub tickets: u64,
    /// Servers that added Quack.
    pub joins: u64,
    /// Servers that removed Quack.
    pub leaves: u64,
    /// Job runs this process finished (over [`Trend::runs_seconds`]).
    pub job_runs: u64,
    /// Of those, runs that failed (retried or dead).
    pub job_failures: u64,
    /// How far back the job runs go: 24 hours, or less after a restart.
    pub runs_seconds: u64,
    /// Cases in each of the last 24 clock hours, oldest first.
    pub cases_hourly: Vec<u64>,
    /// Job runs and failed runs in each of the last 24 hours of this
    /// process, oldest first.
    pub runs_hourly: Vec<(u64, u64)>,
}

/// What the status boards show (§21.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The version.
    pub version: String,
    /// The commit, when the build set one.
    pub commit: Option<String>,
    /// When this run started.
    pub started_at: OffsetDateTime,
    /// Whether the gateway is connected.
    pub gateway_connected: bool,
    /// The last heartbeat round trip, in milliseconds.
    pub latency_ms: Option<u64>,
    /// Servers Quack is in.
    pub servers: u64,
    /// Members across them (where known).
    pub members: u64,
    /// The last 24 hours.
    pub trend: Trend,
    /// Job kinds with work waiting.
    pub backlog: Vec<Backlog>,
    /// Dead jobs.
    pub dead_jobs: u64,
    /// Failed actions waiting for staff.
    pub failed_actions: u64,
    /// Appeals waiting for staff.
    pub open_appeals: u64,
    /// Open tickets.
    pub open_tickets: u64,
    /// Database connections in use.
    pub pool_used: u32,
    /// The most connections the pool may open.
    pub pool_size: u32,
    /// When this was read.
    pub updated_at: OffsetDateTime,
}

/// A ready job older than this makes the boards amber.
const SLOW_QUEUE_SECONDS: u64 = 60;
/// A ready job older than this is a public "Delayed".
const DELAYED_SECONDS: u64 = 600;

impl Status {
    fn oldest_wait(&self) -> u64 {
        self.backlog
            .iter()
            .map(|b| b.oldest_seconds)
            .max()
            .unwrap_or(0)
    }

    fn waiting(&self) -> u64 {
        self.backlog.iter().map(|b| b.ready).sum()
    }

    fn pool_full(&self) -> bool {
        self.pool_size > 0 && self.pool_used >= self.pool_size
    }

    fn uptime(&self) -> u64 {
        u64::try_from((self.updated_at - self.started_at).whole_seconds())
            .unwrap_or(0)
            .max(1)
    }

    /// What needs a look, for the ops board.
    fn concerns(&self) -> Vec<String> {
        let mut concerns = Vec::new();
        if !self.gateway_connected {
            concerns.push("Discord isn't connected".to_owned());
        }
        if self.dead_jobs > 0 {
            concerns.push(fmt::plural(self.dead_jobs, "dead job", "dead jobs"));
        }
        if self.failed_actions > 0 {
            concerns.push(fmt::plural(
                self.failed_actions,
                "failed action",
                "failed actions",
            ));
        }
        if self.oldest_wait() >= SLOW_QUEUE_SECONDS {
            concerns.push("a slow queue".to_owned());
        }
        if self.pool_full() {
            concerns.push("a full database pool".to_owned());
        }
        concerns
    }

    fn tone(&self) -> Tone {
        if !self.gateway_connected {
            Tone::Danger
        } else if self.concerns().is_empty() {
            Tone::Success
        } else {
            Tone::Warning
        }
    }
}

/// "25 Nov, 09:03 UTC".
fn stamp(at: OffsetDateTime) -> String {
    let at = at.to_offset(UtcOffset::UTC);
    let date = fmt::date(at);
    let day_month = date.rsplit_once(' ').map_or(date.as_str(), |(dm, _)| dm);
    format!("{day_month}, {:02}:{:02} UTC", at.hour(), at.minute())
}

fn ok(good: bool, bad: Tone) -> Tone {
    if good { Tone::Success } else { bad }
}

/// "12h 25m", "3d 4h", "40m": an uptime short enough for a tile.
fn short_duration(seconds: u64) -> String {
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

fn members_note(s: &Status) -> String {
    s.members.checked_div(s.servers).map_or_else(
        || "across servers".to_owned(),
        |each| format!("about {} a server", fmt::compact(each)),
    )
}

fn hours_chart(title: &str, note: String, values: Vec<u64>, failed: Option<Vec<u64>>) -> Section {
    Section::Columns(Columns {
        title: title.to_owned(),
        note: Some(note),
        values,
        failed,
        start: "24 hours ago".to_owned(),
        end: "now".to_owned(),
    })
}

fn queue_check(s: &Status) -> Check {
    let waiting = s.waiting();
    let oldest = s.oldest_wait();
    Check {
        label: "Job queue".to_owned(),
        detail: Some(if waiting == 0 {
            "nothing waiting".to_owned()
        } else {
            format!(
                "{} waiting, oldest {}",
                fmt::count(waiting),
                fmt::duration(oldest.max(1))
            )
        }),
        state: if oldest >= SLOW_QUEUE_SECONDS {
            "Slow"
        } else {
            "Normal"
        }
        .to_owned(),
        tone: ok(oldest < SLOW_QUEUE_SECONDS, Tone::Warning),
    }
}

/// The ops board's health lines.
fn ops_checks(s: &Status) -> Vec<Check> {
    vec![
        Check {
            label: "Discord gateway".to_owned(),
            detail: s
                .latency_ms
                .filter(|_| s.gateway_connected)
                .map(|ms| format!("{ms} ms heartbeat")),
            state: if s.gateway_connected {
                "Connected"
            } else {
                "Disconnected"
            }
            .to_owned(),
            tone: ok(s.gateway_connected, Tone::Danger),
        },
        queue_check(s),
        Check {
            label: "Dead jobs".to_owned(),
            detail: (s.dead_jobs > 0).then(|| "see /ops jobs".to_owned()),
            state: if s.dead_jobs == 0 {
                "None".to_owned()
            } else {
                fmt::count(s.dead_jobs)
            },
            tone: ok(s.dead_jobs == 0, Tone::Danger),
        },
        Check {
            label: "Failed actions".to_owned(),
            detail: (s.failed_actions > 0).then(|| "staff can retry them".to_owned()),
            state: if s.failed_actions == 0 {
                "None".to_owned()
            } else {
                fmt::count(s.failed_actions)
            },
            tone: ok(s.failed_actions == 0, Tone::Warning),
        },
        Check {
            label: "Database".to_owned(),
            detail: Some(format!(
                "{} of {} connections in use",
                fmt::count(s.pool_used),
                fmt::count(s.pool_size)
            )),
            state: if s.pool_full() { "Full" } else { "Normal" }.to_owned(),
            tone: ok(!s.pool_full(), Tone::Warning),
        },
    ]
}

/// The ops board's image (§21.6).
pub fn status_card(s: &Status) -> Card {
    let tone = s.tone();
    let concerns = s.concerns();
    let (title, badge) = match tone {
        Tone::Danger => ("Quack isn't connected to Discord", "Down"),
        Tone::Warning => ("Quack needs a look", "Needs a look"),
        Tone::Success | Tone::Neutral => ("All systems normal", "Healthy"),
    };
    let mut build = s.version.clone();
    if let Some(commit) = &s.commit {
        build = format!("{build} ({})", commit.chars().take(7).collect::<String>());
    }
    let t = &s.trend;
    let since = if t.runs_seconds < 86_400 {
        format!(
            " · since start {} ago",
            fmt::duration(t.runs_seconds.max(1))
        )
    } else {
        String::new()
    };
    let runs_note = format!(
        "{} runs · {} failed{since}",
        fmt::compact(t.job_runs),
        fmt::count(t.job_failures)
    );

    Card {
        title: title.to_owned(),
        subtitle: Some(format!("Quack {build} · up {}", fmt::duration(s.uptime()))),
        tone,
        badge: Some(badge.to_owned()),
        sections: vec![
            Section::Tiles(vec![
                Tile::new("Servers", fmt::count(s.servers)).note(if t.joins + t.leaves == 0 {
                    "no change today".to_owned()
                } else {
                    format!("+{} · −{} today", fmt::count(t.joins), fmt::count(t.leaves))
                }),
                Tile::new("Members", fmt::compact(s.members)).note(members_note(s)),
                Tile::new("Open appeals", fmt::count(s.open_appeals)).note("waiting for staff"),
                Tile::new("Open tickets", fmt::count(s.open_tickets)).note("across servers"),
            ]),
            Section::Checks {
                title: Some("Health".to_owned()),
                rows: ops_checks(s),
            },
            hours_chart(
                "Cases",
                format!(
                    "{} · {} · {} in 24 hours",
                    fmt::plural(t.cases, "case", "cases"),
                    fmt::plural(t.appeals, "appeal", "appeals"),
                    fmt::plural(t.tickets, "ticket", "tickets"),
                ),
                t.cases_hourly.clone(),
                None,
            ),
            hours_chart(
                "Job runs",
                runs_note,
                t.runs_hourly.iter().map(|h| h.0).collect(),
                Some(t.runs_hourly.iter().map(|h| h.1).collect()),
            ),
        ],
        footer: if concerns.is_empty() {
            "Nothing needs a look.".to_owned()
        } else {
            format!("Needs a look: {}.", fmt::list(&concerns, "and"))
        },
        footer_right: Some(format!("Updated {}", stamp(s.updated_at))),
        alt: format!(
            "{title}. {}, {} members, {} in the last 24 hours.",
            fmt::plural(s.servers, "server", "servers"),
            fmt::compact(s.members),
            fmt::plural(t.cases, "case", "cases"),
        ),
    }
}

/// `ops.status`: the ops board, with **Refresh** when `refresh`. The
/// image is [`status_card`], sent as [`STATUS_IMAGE`].
pub fn status(s: &Status, refresh: bool) -> Msg {
    let card = status_card(s);
    let mut msg = Msg::new(card.tone).block(Block::Image {
        name: STATUS_IMAGE.to_owned(),
        alt: card.alt,
    });
    for b in s
        .backlog
        .iter()
        .filter(|b| b.oldest_seconds >= SLOW_QUEUE_SECONDS)
    {
        msg = msg.block(Block::Text(
            Text::fixed(format!("🟡 {} ", fmt::count(b.ready)))
                .push(Inline::Code(b.kind.to_owned()))
                .s(format!(
                    " waiting, oldest {}",
                    fmt::duration(b.oldest_seconds)
                )),
        ));
    }
    if s.dead_jobs > 0 {
        msg = msg.block(Block::Text(
            Text::fixed(format!(
                "🔴 {} · ",
                fmt::plural(s.dead_jobs, "dead job", "dead jobs")
            ))
            .push(Inline::Code("/ops jobs".to_owned())),
        ));
    }
    msg = msg.block(Block::Subtext(
        Text::fixed("Updated ").time(s.updated_at, TimeStyle::Relative),
    ));
    if refresh {
        msg = msg.buttons(vec![Button::new(
            "Refresh",
            ButtonStyle::Secondary,
            ButtonAction::OpsRefresh,
        )]);
    }
    msg
}

/// The public verdict: its tone, title, and badge.
fn public_verdict(s: &Status) -> (Tone, &'static str, &'static str) {
    if !s.gateway_connected {
        (Tone::Danger, "Quack is offline", "Outage")
    } else if s.oldest_wait() >= DELAYED_SECONDS || s.pool_full() {
        (Tone::Warning, "Some actions are delayed", "Degraded")
    } else {
        (Tone::Success, "All systems operational", "Operational")
    }
}

/// The public board's image (§21.6): how each part is doing, servers,
/// members, and uptime; nothing about any one server, no case counts, and
/// nothing staff-only.
pub fn public_card(s: &Status) -> Card {
    let (tone, title, badge) = public_verdict(s);
    let oldest = s.oldest_wait();
    let rows = vec![
        Check {
            label: "Discord connection".to_owned(),
            detail: s
                .latency_ms
                .filter(|_| s.gateway_connected)
                .map(|ms| format!("{ms} ms")),
            state: if s.gateway_connected {
                "Operational"
            } else {
                "Outage"
            }
            .to_owned(),
            tone: ok(s.gateway_connected, Tone::Danger),
        },
        Check {
            label: "Moderation actions".to_owned(),
            detail: Some("timeouts, kicks, bans, DMs".to_owned()),
            state: if oldest >= DELAYED_SECONDS {
                "Delayed"
            } else {
                "Operational"
            }
            .to_owned(),
            tone: ok(oldest < DELAYED_SECONDS, Tone::Warning),
        },
        Check {
            label: "Database".to_owned(),
            detail: None,
            state: if s.pool_full() {
                "Degraded"
            } else {
                "Operational"
            }
            .to_owned(),
            tone: ok(!s.pool_full(), Tone::Warning),
        },
    ];
    Card {
        title: title.to_owned(),
        subtitle: Some(format!("Quack status · {}", stamp(s.updated_at))),
        tone,
        badge: Some(badge.to_owned()),
        sections: vec![
            Section::Checks { title: None, rows },
            Section::Tiles(vec![
                Tile::new("Servers", fmt::count(s.servers)),
                Tile::new("Members", fmt::compact(s.members)),
                Tile::new("Uptime", short_duration(s.uptime())),
            ]),
        ],
        footer: format!("Quack {}", s.version),
        footer_right: Some("Updated every 10 minutes".to_owned()),
        alt: format!(
            "{title}. Quack is in {} with {} members.",
            fmt::plural(s.servers, "server", "servers"),
            fmt::compact(s.members)
        ),
    }
}

/// `ops.public_status`: the public board. The image is [`public_card`],
/// sent as [`PUBLIC_IMAGE`]. It has no buttons: anyone can see it.
pub fn public_status(s: &Status) -> Msg {
    let card = public_card(s);
    Msg::new(card.tone)
        .block(Block::Image {
            name: PUBLIC_IMAGE.to_owned(),
            alt: card.alt,
        })
        .block(Block::Subtext(
            Text::fixed("Updated ")
                .time(s.updated_at, TimeStyle::Relative)
                .s(". If that was more than 20 minutes ago, Quack may be down."),
        ))
}
