//! `ops.status`: the status board (§21.6). A one-line verdict on top,
//! then short sections: Discord, the last 24 hours, the queue, what waits
//! for staff, and the database.

use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{Block, Button, ButtonStyle, Inline, Msg, Text, TimeStyle, Tone};

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
}

/// What the status board shows (§21.6).
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

/// A ready job older than this makes the board amber.
const SLOW_QUEUE_SECONDS: u64 = 60;

const GREEN: &str = "🟢";
const AMBER: &str = "🟡";
const RED: &str = "🔴";

/// The verdict: its tone, dot, and words.
fn verdict(s: &Status) -> (Tone, String) {
    if !s.gateway_connected {
        return (
            Tone::Danger,
            format!("{RED} Quack isn't connected to Discord"),
        );
    }
    let mut concerns = Vec::new();
    if s.dead_jobs > 0 {
        concerns.push(fmt::plural(s.dead_jobs, "dead job", "dead jobs"));
    }
    if s.failed_actions > 0 {
        concerns.push(fmt::plural(
            s.failed_actions,
            "failed action",
            "failed actions",
        ));
    }
    if s.backlog
        .iter()
        .any(|b| b.oldest_seconds >= SLOW_QUEUE_SECONDS)
    {
        concerns.push("a slow queue".to_owned());
    }
    if s.pool_size > 0 && s.pool_used >= s.pool_size {
        concerns.push("a full database pool".to_owned());
    }
    if concerns.is_empty() {
        (Tone::Success, format!("{GREEN} All systems normal"))
    } else {
        (
            Tone::Warning,
            format!("{AMBER} Needs a look: {}", fmt::list(&concerns, "and")),
        )
    }
}

/// A bold label over its lines; every section but the first starts with a
/// blank line, so the sections read apart.
fn section(label: &str, body: Text, first: bool) -> Block {
    let start = if first {
        Text::new()
    } else {
        Text::fixed("\n")
    };
    Block::Text(start.bold(label).s("\n").then(body))
}

fn dot(ok: bool, bad: &'static str) -> &'static str {
    if ok { GREEN } else { bad }
}

/// The line under the verdict: version, commit, uptime, and when.
fn build_line(s: &Status) -> Text {
    let uptime = (s.updated_at - s.started_at).whole_seconds().max(0);
    let mut build = Text::new().push(Inline::Code(s.version.clone()));
    if let Some(commit) = &s.commit {
        build = build
            .s(" ")
            .push(Inline::Code(commit.chars().take(7).collect()));
    }
    build
        .s(format!(
            " · up {} · updated ",
            fmt::duration(u64::try_from(uptime).unwrap_or(0).max(1))
        ))
        .time(s.updated_at, TimeStyle::Relative)
}

fn discord(s: &Status) -> Text {
    Text::fixed(match (s.gateway_connected, s.latency_ms) {
        (true, Some(ms)) => format!("{GREEN} Connected · {ms} ms heartbeat\n"),
        (true, None) => format!("{GREEN} Connected\n"),
        (false, _) => format!("{RED} Not connected\n"),
    })
    .bold(fmt::count(s.servers))
    .s(if s.servers == 1 {
        " server · "
    } else {
        " servers · "
    })
    .bold(fmt::compact(s.members))
    .s(if s.members == 1 {
        " member"
    } else {
        " members"
    })
}

fn last_day(t: &Trend) -> Text {
    let mut day = Text::fixed(format!(
        "{} · {} · {}\n",
        fmt::plural(t.cases, "case", "cases"),
        fmt::plural(t.appeals, "appeal", "appeals"),
        fmt::plural(t.tickets, "ticket", "tickets"),
    ))
    .s(if t.joins + t.leaves == 0 {
        "No servers joined or left\n".to_owned()
    } else {
        format!(
            "+{} joined · −{} left\n",
            fmt::plural(t.joins, "server", "servers"),
            fmt::count(t.leaves)
        )
    })
    .s(format!("{} job runs", fmt::count(t.job_runs)));
    if t.job_failures > 0 {
        day = day.s(format!(", {} failed", fmt::count(t.job_failures)));
    }
    if t.runs_seconds < 86_400 {
        day = day.s(format!(
            " (since start, {} ago)",
            fmt::duration(t.runs_seconds.max(1))
        ));
    }
    day
}

fn queue(s: &Status) -> Text {
    let mut queue = if s.backlog.is_empty() {
        Text::fixed(format!("{GREEN} Nothing waiting"))
    } else {
        let mut q = Text::new();
        for (i, b) in s.backlog.iter().enumerate() {
            if i > 0 {
                q = q.s("\n");
            }
            q = q
                .s(format!(
                    "{} {} ",
                    dot(b.oldest_seconds < SLOW_QUEUE_SECONDS, AMBER),
                    fmt::count(b.ready)
                ))
                .push(Inline::Code(b.kind.to_owned()))
                .s(format!(
                    " waiting, oldest {}",
                    fmt::duration(b.oldest_seconds.max(1))
                ));
        }
        q
    };
    queue = if s.dead_jobs == 0 {
        queue.s(format!("\n{GREEN} No dead jobs"))
    } else {
        queue
            .s(format!(
                "\n{RED} {} · ",
                fmt::plural(s.dead_jobs, "dead job", "dead jobs")
            ))
            .push(Inline::Code("/ops jobs".to_owned()))
    };
    queue
}

/// `ops.status`: the status board, with **Refresh** when `refresh`.
pub fn status(s: &Status, refresh: bool) -> Msg {
    let (tone, headline) = verdict(s);
    let staff = Text::fixed(format!(
        "{} {} · {} · {}",
        dot(s.failed_actions == 0, AMBER),
        fmt::plural(s.failed_actions, "failed action", "failed actions"),
        fmt::plural(s.open_appeals, "open appeal", "open appeals"),
        fmt::plural(s.open_tickets, "open ticket", "open tickets"),
    ));
    let database = Text::fixed(format!(
        "{} {} of {} connections in use",
        dot(s.pool_size == 0 || s.pool_used < s.pool_size, AMBER),
        fmt::count(s.pool_used),
        fmt::count(s.pool_size)
    ));
    let mut msg = Msg::new(tone)
        .block(Block::Heading(Text::fixed(headline)))
        .block(Block::Subtext(build_line(s)))
        .block(Block::Divider)
        .block(section("Discord", discord(s), true))
        .block(section("Last 24 hours", last_day(&s.trend), false))
        .block(section("Queue", queue(s), false))
        .block(section("Waiting for staff", staff, false))
        .block(section("Database", database, false));
    if refresh {
        msg = msg.buttons(vec![Button::new(
            "Refresh",
            ButtonStyle::Secondary,
            ButtonAction::OpsRefresh,
        )]);
    }
    msg
}
