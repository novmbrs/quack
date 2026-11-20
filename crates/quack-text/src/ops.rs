//! Ops messages (§12.15, §21): what Quack posts in the ops server and
//! answers to `/ops`. Only Quack's operators read these, so they may name
//! machine codes (job kinds, failure codes) in code spans.

use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::{AccountId, GuildId, JobId};
use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{Block, Button, ButtonStyle, Inline, Msg, Person, Text, TimeStyle, Tone};
use crate::word;

/// How the run before this one ended (§21.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Previous {
    /// Ops knows of no run before (a new database, or ops just turned on).
    None,
    /// It stopped cleanly.
    Stopped {
        /// When.
        at: OffsetDateTime,
        /// Why.
        reason: StopReason,
    },
    /// It ended without stopping (a crash or a kill).
    Ended {
        /// Its last heartbeat.
        seen_at: OffsetDateTime,
    },
}

/// Why a run stopped cleanly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// A signal (a deploy or a restart).
    Signal,
    /// The gateway closed for good.
    GatewayClosed,
}

/// What `ops.started` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    /// The version.
    pub version: String,
    /// The commit, when the build set one.
    pub commit: Option<String>,
    /// When this run started.
    pub started_at: OffsetDateTime,
    /// Seconds from start to ready.
    pub ready_in_seconds: u64,
    /// Migrations this start applied.
    pub migrations_applied: usize,
    /// Servers Quack is in.
    pub servers: u64,
    /// The run before.
    pub previous: Previous,
}

/// `ops.started`: a new run is ready.
pub fn started(s: &Started) -> Msg {
    let mut version = Text::new().push(Inline::Code(s.version.clone()));
    if let Some(commit) = &s.commit {
        version = version
            .s(" (")
            .push(Inline::Code(fmt::cut(commit, 12)))
            .s(")");
    }
    let (tone, previous) = match s.previous {
        Previous::None => (
            Tone::Success,
            Text::fixed("This is the first run ops knows about."),
        ),
        Previous::Stopped { at, reason } => (
            if reason == StopReason::Signal {
                Tone::Success
            } else {
                Tone::Warning
            },
            Text::fixed("The run before stopped cleanly ")
                .time(at, TimeStyle::Relative)
                .s(match reason {
                    StopReason::Signal => " (a restart or deploy).",
                    StopReason::GatewayClosed => " because Discord's gateway closed for good.",
                }),
        ),
        Previous::Ended { seen_at } => {
            let down = (s.started_at - seen_at).whole_seconds().max(0);
            let down = u64::try_from(down).unwrap_or(0);
            (
                Tone::Warning,
                Text::fixed(
                    "The run before ended without stopping (a crash or a kill). It was last seen ",
                )
                .time(seen_at, TimeStyle::Relative)
                .s(format!(
                    ", so Quack was down for about {}.",
                    fmt::duration(down.max(1))
                )),
            )
        }
    };
    Msg::new(tone)
        .block(Block::Heading(Text::fixed("Quack started")))
        .block(Block::Field {
            label: "Version".to_owned(),
            value: version,
        })
        .block(Block::Field {
            label: "Ready in".to_owned(),
            value: Text::fixed(fmt::duration(s.ready_in_seconds.max(1))),
        })
        .block(Block::Field {
            label: "Migrations applied".to_owned(),
            value: Text::fixed(fmt::count(u64::try_from(s.migrations_applied).unwrap_or(0))),
        })
        .block(Block::Field {
            label: "Servers".to_owned(),
            value: Text::fixed(fmt::count(s.servers)),
        })
        .para(previous)
}

/// Gateway trouble over one window (§21.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gateway {
    /// Live connections that dropped.
    pub drops: u64,
    /// Failed reconnect attempts.
    pub connect_failures: u64,
    /// New sessions.
    pub identifies: u64,
    /// Resumed sessions.
    pub resumes: u64,
    /// Sessions Discord invalidated.
    pub invalid_sessions: u64,
    /// Whether the gateway is connected now.
    pub connected_now: bool,
}

/// `ops.gateway`: what the gateway did in the last 10 minutes.
pub fn gateway(g: &Gateway) -> Msg {
    let mut parts = Vec::new();
    let mut add = |n: u64, one: &str, many: &str| {
        if n > 0 {
            parts.push(format!(
                "{} {}",
                fmt::count(n),
                if n == 1 { one } else { many }
            ));
        }
    };
    add(g.drops, "dropped connection", "dropped connections");
    add(g.connect_failures, "failed reconnect", "failed reconnects");
    add(g.resumes, "resumed session", "resumed sessions");
    add(g.identifies, "new session", "new sessions");
    add(
        g.invalid_sessions,
        "invalidated session",
        "invalidated sessions",
    );
    let summary = if parts.is_empty() {
        "Nothing changed.".to_owned()
    } else {
        format!("{}.", capitalize(&fmt::list(&parts, "and")))
    };
    let (tone, now) = if g.connected_now {
        (Tone::Warning, "Quack is connected now.")
    } else {
        (Tone::Danger, "Quack is not connected right now.")
    };
    Msg::new(tone)
        .block(Block::Heading(Text::fixed(
            "Gateway trouble in the last 10 minutes",
        )))
        .para(Text::fixed(summary))
        .para(Text::fixed(now))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// What happened to Quack's membership of a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallChange {
    /// Added for the first time.
    Joined,
    /// Added again after leaving.
    Rejoined,
    /// Removed.
    Left,
}

/// What `ops.install` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    /// The change.
    pub change: InstallChange,
    /// The server's name.
    pub name: String,
    /// The server.
    pub guild: GuildId,
    /// Its owner.
    pub owner: AccountId,
    /// Servers Quack is in now.
    pub servers: u64,
}

/// `ops.install`: Quack joined, rejoined, or left a server.
pub fn install(i: &Install) -> Msg {
    let (tone, heading, verb) = match i.change {
        InstallChange::Joined => (Tone::Success, "Quack joined a server", "was added to"),
        InstallChange::Rejoined => (
            Tone::Success,
            "Quack rejoined a server",
            "was added again to",
        ),
        InstallChange::Left => (Tone::Warning, "Quack left a server", "was removed from"),
    };
    Msg::new(tone)
        .block(Block::Heading(Text::fixed(heading)))
        .para(
            Text::fixed(format!("Quack {verb} "))
                .bold_user(&i.name)
                .s(" (")
                .push(Inline::Code(i.guild.to_string()))
                .s(")."),
        )
        .block(Block::Field {
            label: "Owner".to_owned(),
            value: Text::new().person(&Person::unnamed(i.owner)),
        })
        .block(Block::Subtext(Text::fixed(format!(
            "Quack is in {} now.",
            fmt::plural(i.servers, "server", "servers")
        ))))
}

/// A dead job as ops shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadJob {
    /// The job.
    pub id: JobId,
    /// Its kind's name (`run_action`).
    pub kind: &'static str,
    /// Its key.
    pub key: String,
    /// Its server.
    pub guild: Option<GuildId>,
    /// Runs it used.
    pub attempts: u32,
    /// Its last error code.
    pub error: Option<String>,
    /// When it died.
    pub dead_at: OffsetDateTime,
}

fn job_fields(msg: Msg, j: &DeadJob) -> Msg {
    let msg = msg
        .block(Block::Field {
            label: "Key".to_owned(),
            value: Text::new().push(Inline::Code(j.key.clone())),
        })
        .block(Block::Field {
            label: "Server".to_owned(),
            value: j.guild.map_or_else(
                || Text::fixed("none"),
                |g| Text::new().push(Inline::Code(g.to_string())),
            ),
        })
        .block(Block::Field {
            label: "Runs".to_owned(),
            value: Text::fixed(fmt::count(j.attempts)),
        });
    let msg = match &j.error {
        Some(e) => msg.block(Block::Field {
            label: "Last error".to_owned(),
            value: Text::new().push(Inline::Code(fmt::cut(&fmt::one_line(e), 200))),
        }),
        None => msg,
    };
    msg.block(Block::Subtext(
        Text::fixed("Died ")
            .time(j.dead_at, TimeStyle::FullRelative)
            .s("."),
    ))
}

fn job_buttons(j: &DeadJob) -> Vec<Button> {
    vec![
        Button::new(
            "Requeue",
            ButtonStyle::Primary,
            ButtonAction::OpsRequeueJob(j.id),
        ),
        Button::new(
            "Dismiss",
            ButtonStyle::Secondary,
            ButtonAction::OpsDismissJob(j.id),
        ),
    ]
}

/// `ops.dead_job`: a job ran out of attempts.
pub fn dead_job(j: &DeadJob) -> Msg {
    job_fields(
        Msg::new(Tone::Danger).block(Block::Heading(
            Text::fixed("A ")
                .push(Inline::Code(j.kind.to_owned()))
                .s(" job died"),
        )),
        j,
    )
    .buttons(job_buttons(j))
}

/// What an operator did with a dead job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handled {
    /// It runs again now.
    Requeued,
    /// It was deleted.
    Dismissed,
}

/// `ops.dead_job_handled`: the alert after Requeue or Dismiss.
pub fn dead_job_handled(j: &DeadJob, how: Handled, by: AccountId) -> Msg {
    let (tone, did) = match how {
        Handled::Requeued => (Tone::Success, " requeued it. It runs again now."),
        Handled::Dismissed => (Tone::Neutral, " dismissed it. The job is deleted."),
    };
    job_fields(
        Msg::new(tone).block(Block::Heading(
            Text::fixed("A ")
                .push(Inline::Code(j.kind.to_owned()))
                .s(" job died"),
        )),
        j,
    )
    .para(Text::new().person(&Person::unnamed(by)).s(did))
}

/// `ops.dead_jobs_more`: more jobs died this minute than were posted.
pub fn dead_jobs_more(more: u64) -> Msg {
    Msg::text(
        Tone::Danger,
        Text::fixed(format!(
            "{} more died in the same minute. Use ",
            fmt::count(more)
        ))
        .push(Inline::Code("/ops jobs".to_owned()))
        .s(" to see them all."),
    )
}

/// `ops.job_gone`: the job isn't dead any more.
pub fn job_gone() -> Text {
    Text::fixed(
        "That job isn't dead any more: someone requeued or dismissed it, or housekeeping deleted it.",
    )
}

/// Failed actions of one kind and reason in one server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailedGroup {
    /// The server.
    pub guild: GuildId,
    /// The action kind.
    pub kind: ActionKind,
    /// Why they failed.
    pub reason: ActionFailure,
    /// How many.
    pub count: u64,
}

/// The most groups `ops.failed_actions` lists.
pub const FAILED_GROUPS_SHOWN: usize = 10;

/// `ops.failed_actions`: actions failed in the last minute.
pub fn failed_actions(groups: &[FailedGroup]) -> Msg {
    let total: u64 = groups.iter().map(|g| g.count).sum();
    let mut msg = Msg::new(Tone::Warning)
        .block(Block::Heading(Text::fixed(format!(
            "{} failed",
            fmt::plural(total, "action", "actions")
        ))))
        .para(Text::fixed(
            "Staff can retry or dismiss them in their server. Nothing here needs a click.",
        ));
    for g in groups.iter().take(FAILED_GROUPS_SHOWN) {
        msg = msg.para(
            Text::fixed(format!(
                "{} × {} in ",
                fmt::count(g.count),
                word::action(g.kind)
            ))
            .push(Inline::Code(g.guild.to_string()))
            .s(": ")
            .push(Inline::Code(g.reason.as_str().to_owned())),
        );
    }
    let hidden = groups.len().saturating_sub(FAILED_GROUPS_SHOWN);
    if hidden > 0 {
        msg = msg.block(Block::Subtext(Text::fixed(format!(
            "And {} more {}.",
            fmt::count(u64::try_from(hidden).unwrap_or(0)),
            if hidden == 1 { "group" } else { "groups" }
        ))));
    }
    msg
}

/// One group of the error feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorGroup {
    /// Where it was logged.
    pub target: String,
    /// The log line.
    pub message: String,
    /// How often in the minute.
    pub count: u64,
    /// The first trace id, when exported.
    pub trace_id: Option<String>,
    /// A link to that trace, when `QUACK_OPS_TRACE_URL` is set.
    pub trace_url: Option<String>,
}

/// The most groups `ops.errors` lists.
pub const ERROR_GROUPS_SHOWN: usize = 10;

/// `ops.errors`: the error feed for one minute.
pub fn errors(groups: &[ErrorGroup], dropped: u64) -> Msg {
    let total: u64 = groups.iter().map(|g| g.count).sum::<u64>() + dropped;
    let mut msg = Msg::new(Tone::Danger).block(Block::Heading(Text::fixed(format!(
        "{} in the last minute",
        fmt::plural(total, "error", "errors")
    ))));
    for g in groups.iter().take(ERROR_GROUPS_SHOWN) {
        let mut line = Text::new()
            .push(Inline::Code(fmt::cut(&g.target, 80)))
            .s(" ")
            .push(Inline::Code(fmt::cut(&fmt::one_line(&g.message), 300)));
        if g.count > 1 {
            line = line.s(format!(" ×{}", fmt::count(g.count)));
        }
        match (&g.trace_url, &g.trace_id) {
            (Some(url), _) => line = line.s(" ").link("trace", url.clone()),
            (None, Some(id)) => line = line.s(" trace ").push(Inline::Code(id.clone())),
            (None, None) => {}
        }
        msg = msg.para(line);
    }
    let hidden = groups.len().saturating_sub(ERROR_GROUPS_SHOWN);
    if hidden > 0 {
        msg = msg.block(Block::Subtext(Text::fixed(format!(
            "And {} more {}.",
            fmt::count(u64::try_from(hidden).unwrap_or(0)),
            if hidden == 1 {
                "kind of error"
            } else {
                "kinds of error"
            }
        ))));
    }
    if dropped > 0 {
        msg = msg.block(Block::Subtext(Text::fixed(format!(
            "{} didn't fit in the buffer; the logs have them.",
            fmt::count(dropped)
        ))));
    }
    msg
}

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

/// What the status board shows (§21.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The version.
    pub version: String,
    /// When this run started.
    pub started_at: OffsetDateTime,
    /// Whether the gateway is connected.
    pub gateway_connected: bool,
    /// The last heartbeat round trip, in milliseconds.
    pub latency_ms: Option<u64>,
    /// Servers Quack is in.
    pub servers: u64,
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
    /// Database connections open.
    pub pool_size: u32,
    /// When this was read.
    pub updated_at: OffsetDateTime,
}

/// `ops.status`: the status board, with **Refresh** when `refresh`.
pub fn status(s: &Status, refresh: bool) -> Msg {
    let healthy = s.gateway_connected && s.dead_jobs == 0;
    let mut msg = Msg::new(if healthy {
        Tone::Success
    } else {
        Tone::Warning
    })
    .block(Block::Heading(Text::fixed("Quack status")))
    .block(Block::Field {
        label: "Version".to_owned(),
        value: Text::new()
            .push(Inline::Code(s.version.clone()))
            .s(", started ")
            .time(s.started_at, TimeStyle::Relative),
    })
    .block(Block::Field {
        label: "Gateway".to_owned(),
        value: Text::fixed(match (s.gateway_connected, s.latency_ms) {
            (true, Some(ms)) => format!("connected, {ms} ms heartbeat"),
            (true, None) => "connected".to_owned(),
            (false, _) => "not connected".to_owned(),
        }),
    })
    .block(Block::Field {
        label: "Servers".to_owned(),
        value: Text::fixed(fmt::count(s.servers)),
    });
    let jobs = if s.backlog.is_empty() {
        Text::fixed("nothing waiting")
    } else {
        let parts: Vec<String> = s
            .backlog
            .iter()
            .map(|b| {
                format!(
                    "{} {} (oldest {})",
                    fmt::count(b.ready),
                    b.kind,
                    fmt::duration(b.oldest_seconds.max(1))
                )
            })
            .collect();
        Text::fixed(parts.join(", "))
    };
    msg = msg
        .block(Block::Field {
            label: "Jobs ready".to_owned(),
            value: jobs,
        })
        .block(Block::Field {
            label: "Dead jobs".to_owned(),
            value: Text::fixed(fmt::count(s.dead_jobs)),
        })
        .block(Block::Field {
            label: "Failed actions".to_owned(),
            value: Text::fixed(fmt::count(s.failed_actions)),
        })
        .block(Block::Field {
            label: "Open appeals".to_owned(),
            value: Text::fixed(fmt::count(s.open_appeals)),
        })
        .block(Block::Field {
            label: "Open tickets".to_owned(),
            value: Text::fixed(fmt::count(s.open_tickets)),
        })
        .block(Block::Field {
            label: "Database".to_owned(),
            value: Text::fixed(format!(
                "{} of {} connections in use",
                fmt::count(s.pool_used),
                fmt::count(s.pool_size)
            )),
        })
        .block(Block::Subtext(
            Text::fixed("Updated ")
                .time(s.updated_at, TimeStyle::Relative)
                .s("."),
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

/// What `/ops server` shows (§21.1): metadata and counts only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// The server.
    pub guild: GuildId,
    /// Its name.
    pub name: String,
    /// Its owner.
    pub owner: AccountId,
    /// Whether Quack is in it.
    pub installed: bool,
    /// When Quack first joined.
    pub first_joined_at: OffsetDateTime,
    /// When Quack left, while not installed.
    pub left_at: Option<OffsetDateTime>,
    /// Cases, voided included.
    pub cases: u64,
    /// Voided cases.
    pub voided_cases: u64,
    /// Active rules.
    pub active_rules: u64,
    /// Appeals waiting.
    pub open_appeals: u64,
    /// Failed actions waiting.
    pub failed_actions: u64,
    /// Open tickets.
    pub open_tickets: u64,
    /// The modules that are on, by name.
    pub modules_on: Vec<&'static str>,
    /// Quack's missing permissions (`None`: Discord didn't answer).
    pub missing_permissions: Option<Vec<String>>,
}

/// `ops.server`: one server's metadata and counts.
pub fn server(s: &Server) -> Msg {
    let installed = if s.installed {
        Text::fixed("Quack is in this server. It first joined ")
            .time(s.first_joined_at, TimeStyle::Date)
            .s(".")
    } else {
        let t = Text::fixed("Quack isn't in this server any more. It first joined ")
            .time(s.first_joined_at, TimeStyle::Date);
        match s.left_at {
            Some(at) => t.s(" and left ").time(at, TimeStyle::Date).s("."),
            None => t.s("."),
        }
    };
    let modules = if s.modules_on.is_empty() {
        "none".to_owned()
    } else {
        s.modules_on.join(", ")
    };
    let missing = match &s.missing_permissions {
        None => Text::fixed("Discord didn't answer, so Quack couldn't check."),
        Some(p) if p.is_empty() => Text::fixed("none"),
        Some(p) => Text::fixed(p.join(", ")),
    };
    Msg::new(Tone::Neutral)
        .block(Block::Heading(Text::new().bold_user(&s.name)))
        .para(installed)
        .block(Block::Field {
            label: "ID".to_owned(),
            value: Text::new().push(Inline::Code(s.guild.to_string())),
        })
        .block(Block::Field {
            label: "Owner".to_owned(),
            value: Text::new()
                .person(&Person::unnamed(s.owner))
                .s(" (")
                .push(Inline::Code(s.owner.to_string()))
                .s(")"),
        })
        .block(Block::Field {
            label: "Cases".to_owned(),
            value: Text::fixed(format!(
                "{} ({} voided)",
                fmt::count(s.cases),
                fmt::count(s.voided_cases)
            )),
        })
        .block(Block::Field {
            label: "Active rules".to_owned(),
            value: Text::fixed(fmt::count(s.active_rules)),
        })
        .block(Block::Field {
            label: "Waiting for staff".to_owned(),
            value: Text::fixed(format!(
                "{}, {}, {}",
                fmt::plural(s.open_appeals, "appeal", "appeals"),
                fmt::plural(s.failed_actions, "failed action", "failed actions"),
                fmt::plural(s.open_tickets, "open ticket", "open tickets"),
            )),
        })
        .block(Block::Field {
            label: "Modules on".to_owned(),
            value: Text::fixed(modules),
        })
        .block(Block::Field {
            label: "Missing permissions".to_owned(),
            value: missing,
        })
        .block(Block::Subtext(Text::fixed(
            "Ops shows only facts about the server and counts, never its cases or members.",
        )))
}

/// `ops.server_unknown`: no stored server has that id.
pub fn server_unknown(id: &str) -> Text {
    Text::fixed("Quack has never been in a server with the ID ")
        .push(Inline::Code(fmt::cut(id.trim(), 30)))
        .s(".")
}

/// Dead jobs shown per `/ops jobs` page.
pub const JOBS_PER_PAGE: u32 = 5;

/// `ops.jobs`: a page of dead jobs.
pub fn jobs(jobs: &[DeadJob], page: u32, total: u64) -> Msg {
    if total == 0 {
        return Msg::text(Tone::Success, Text::fixed("No dead jobs."));
    }
    let pages = u32::try_from(total.div_ceil(u64::from(JOBS_PER_PAGE))).unwrap_or(u32::MAX);
    let mut msg = Msg::new(Tone::Warning).block(Block::Heading(Text::fixed(format!(
        "{}, newest first",
        fmt::plural(total, "dead job", "dead jobs")
    ))));
    let first = (page.saturating_sub(1)) * JOBS_PER_PAGE;
    let mut buttons = Vec::new();
    for (i, j) in (first + 1..).zip(jobs) {
        let mut line = Text::fixed(format!("{i}. "))
            .push(Inline::Code(j.kind.to_owned()))
            .s(" ")
            .push(Inline::Code(j.key.clone()))
            .s(", died ")
            .time(j.dead_at, TimeStyle::Relative);
        if let Some(e) = &j.error {
            line = line
                .s(": ")
                .push(Inline::Code(fmt::cut(&fmt::one_line(e), 120)));
        }
        msg = msg.para(line);
        buttons.push(vec![
            Button::new(
                format!("Requeue {i}"),
                ButtonStyle::Primary,
                ButtonAction::OpsRequeueJob(j.id),
            ),
            Button::new(
                format!("Dismiss {i}"),
                ButtonStyle::Secondary,
                ButtonAction::OpsDismissJob(j.id),
            ),
        ]);
    }
    msg = msg.block(Block::Subtext(Text::fixed(word::page(page, pages.max(1)))));
    for row in buttons {
        msg = msg.buttons(row);
    }
    msg.buttons(word::pager(page, pages, ButtonAction::OpsJobsPage))
}

/// `ops.not_allowed`: someone who isn't an operator used ops.
pub fn not_allowed() -> Text {
    Text::fixed("Only Quack's operators can use this, and only in the ops server.")
}
