//! `/ops metrics` (§21.8): the fleet at a glance, the servers ranked, and
//! the job queue. Counts and server names only (§21.1).

use std::time::Duration;

use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::GuildId;
use time::OffsetDateTime;

use super::chart::{bar, spark, table};
use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{Block, Button, ButtonStyle, Inline, Msg, Text, TimeStyle, Tone};
use crate::word;

/// A count over the last day, week, and month, and all time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// The last 24 hours.
    pub day: u64,
    /// The last 7 days.
    pub week: u64,
    /// The last 30 days.
    pub month: u64,
    /// All time.
    pub all: u64,
}

/// The installed servers, their members, and module use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fleet {
    /// Servers Quack is in.
    pub servers: u64,
    /// Of those, servers whose member count is known.
    pub counted: u64,
    /// Members across the counted servers.
    pub members: u64,
    /// The median counted server's members.
    pub median_members: Option<u64>,
    /// Counted servers under 100, 1,000, 10,000, and 100,000 members, and
    /// larger.
    pub sizes: [u64; 5],
    /// Servers with tickets on.
    pub tickets_on: u64,
    /// Servers with the server activity log on.
    pub activity_log_on: u64,
    /// Servers with the honeypot on.
    pub honeypot_on: u64,
}

/// What `/ops metrics overview` shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricsOverview {
    /// Servers and members.
    pub fleet: Fleet,
    /// Cases created.
    pub cases: Window,
    /// Cases voided.
    pub voided: Window,
    /// Appeals sent.
    pub appeals: Window,
    /// Tickets opened.
    pub tickets: Window,
    /// Actions done.
    pub actions_done: Window,
    /// Actions failed and not dismissed.
    pub actions_failed: Window,
    /// Servers that added Quack.
    pub joins: Window,
    /// Servers that removed Quack.
    pub leaves: Window,
    /// Cases per UTC day, oldest first, today last.
    pub daily_cases: Vec<u64>,
    /// When this was read.
    pub updated_at: OffsetDateTime,
}

/// A count that fits a five-character column: "9,999", then "12.3K".
fn cell(n: u64) -> String {
    if n < 10_000 {
        fmt::count(n)
    } else {
        fmt::compact(n)
    }
}

fn window_row(label: &str, w: Window) -> Vec<String> {
    vec![
        label.to_owned(),
        cell(w.day),
        cell(w.week),
        cell(w.month),
        cell(w.all),
    ]
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0%".to_owned();
    }
    format!(
        "{}%",
        (u128::from(part) * 100 + u128::from(whole) / 2) / u128::from(whole)
    )
}

const BAR: usize = 12;
/// The module bars are shorter, to leave room for "Server activity log".
const MODULE_BAR: usize = 10;

/// `ops.metrics_overview`: the fleet at a glance.
pub fn metrics_overview(m: &MetricsOverview) -> Msg {
    let f = &m.fleet;
    let mut servers = Text::new()
        .bold(fmt::count(f.servers))
        .s(if f.servers == 1 {
            " server · "
        } else {
            " servers · "
        })
        .bold(fmt::compact(f.members))
        .s(if f.members == 1 {
            " member"
        } else {
            " members"
        });
    if let Some(median) = f.median_members {
        servers = servers.s(" · median ").bold(fmt::count(median));
    }
    if f.counted < f.servers {
        servers = servers.s(format!(
            "\nMember counts are known for {} of them so far.",
            fmt::count(f.counted)
        ));
    }

    let days = m.daily_cases.len();
    let today = m.daily_cases.last().copied().unwrap_or(0);
    let peak = m.daily_cases.iter().copied().max().unwrap_or(0);
    let trend = format!(
        "{}\ntoday {} · peak {} a day",
        spark(&m.daily_cases),
        fmt::count(today),
        fmt::count(peak)
    );

    let activity = table(&[
        vec![
            String::new(),
            "24h".to_owned(),
            "7d".to_owned(),
            "30d".to_owned(),
            "all".to_owned(),
        ],
        window_row("Cases", m.cases),
        window_row("Voided", m.voided),
        window_row("Appeals", m.appeals),
        window_row("Tickets", m.tickets),
        window_row("Actions", m.actions_done),
        window_row("Failed", m.actions_failed),
        window_row("Joined", m.joins),
        window_row("Left", m.leaves),
    ]);

    let size_labels = ["< 100", "100–999", "1K–9.9K", "10K–99K", "100K+"];
    let biggest = f.sizes.iter().copied().max().unwrap_or(0);
    let sizes = table(
        &size_labels
            .iter()
            .zip(f.sizes)
            .map(|(label, n)| vec![(*label).to_owned(), bar(n, biggest, BAR), cell(n)])
            .collect::<Vec<_>>(),
    );

    let modules = table(
        &[
            ("Tickets", f.tickets_on),
            ("Server activity log", f.activity_log_on),
            ("Honeypot", f.honeypot_on),
        ]
        .into_iter()
        .map(|(label, n)| {
            vec![
                label.to_owned(),
                bar(n, f.servers, MODULE_BAR),
                percent(n, f.servers),
                cell(n),
            ]
        })
        .collect::<Vec<_>>(),
    );

    Msg::new(Tone::Neutral)
        .block(Block::Heading(Text::fixed("📊 Quack at a glance")))
        .para(servers)
        .block(Block::Divider)
        .block(Block::Text(Text::new().bold(format!(
            "Cases, last {} days",
            fmt::count(u64::try_from(days).unwrap_or(0))
        ))))
        .block(Block::Code(trend))
        .block(Block::Text(Text::new().bold("Activity")))
        .block(Block::Code(activity))
        .block(Block::Text(Text::new().bold("Server sizes (members)")))
        .block(Block::Code(sizes))
        .block(Block::Text(Text::new().bold("Modules on")))
        .block(Block::Code(modules))
        .block(Block::Subtext(
            Text::fixed("Counts across every server Quack is in, read ")
                .time(m.updated_at, TimeStyle::Relative)
                .s("."),
        ))
}

/// How `/ops metrics servers` orders the servers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ServerSort {
    /// Most members first.
    #[default]
    Members,
    /// Most cases in the last 30 days first.
    Cases,
    /// Newest first install first.
    Newest,
}

impl ServerSort {
    /// Every order, as the buttons show them.
    pub const ALL: [Self; 3] = [Self::Members, Self::Cases, Self::Newest];

    const fn label(self) -> &'static str {
        match self {
            Self::Members => "Members",
            Self::Cases => "Cases",
            Self::Newest => "Newest",
        }
    }

    const fn heading(self) -> &'static str {
        match self {
            Self::Members => "🏆 Servers by members",
            Self::Cases => "🏆 Servers by cases in the last 30 days",
            Self::Newest => "🏆 Newest servers",
        }
    }
}

/// One installed server in the ranking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedServer {
    /// The server.
    pub guild: GuildId,
    /// Its name.
    pub name: String,
    /// Its members, when known.
    pub members: Option<u64>,
    /// Cases in the last 30 days.
    pub cases_month: u64,
    /// Cases, all time.
    pub cases: u64,
    /// When Quack first joined.
    pub first_joined_at: OffsetDateTime,
}

/// Servers per `/ops metrics servers` page.
pub const SERVERS_PER_PAGE: u32 = 10;

/// `ops.metrics_servers`: one page of servers in `sort` order.
pub fn metrics_servers(servers: &[RankedServer], sort: ServerSort, page: u32, total: u64) -> Msg {
    let pages = u32::try_from(total.div_ceil(u64::from(SERVERS_PER_PAGE)))
        .unwrap_or(u32::MAX)
        .max(1);
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(Text::fixed(sort.heading())))
        .block(Block::Subtext(Text::fixed(format!(
            "{} · {}",
            fmt::plural(total, "server", "servers"),
            word::page(page, pages)
        ))));
    if servers.is_empty() {
        msg = msg.para(Text::fixed("No servers on this page."));
    }
    let first = page.saturating_sub(1).saturating_mul(SERVERS_PER_PAGE);
    for (i, s) in (u64::from(first) + 1..).zip(servers) {
        let members = s.members.map_or_else(
            || "members unknown".to_owned(),
            |n| fmt::plural(n, "member", "members"),
        );
        msg = msg
            .para(
                Text::new()
                    .bold(format!("{i}."))
                    .s(" ")
                    .bold_user(fmt::cut(&s.name, 40))
                    .s(" ")
                    .push(Inline::Code(s.guild.to_string())),
            )
            .block(Block::Subtext(
                Text::fixed(format!(
                    "{members} · {} in 30 days, {} in all · since ",
                    fmt::plural(s.cases_month, "case", "cases"),
                    fmt::count(s.cases)
                ))
                .time(s.first_joined_at, TimeStyle::Date),
            ));
    }
    let sorts = ServerSort::ALL
        .into_iter()
        .map(|o| {
            Button::new(
                o.label(),
                if o == sort {
                    ButtonStyle::Primary
                } else {
                    ButtonStyle::Secondary
                },
                ButtonAction::OpsServersPage(o, 1),
            )
        })
        .collect();
    msg.buttons(sorts).buttons(word::pager(page, pages, |p| {
        ButtonAction::OpsServersPage(sort, p)
    }))
}

/// One job kind's runs over the tally's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobRuns {
    /// The kind's name.
    pub kind: &'static str,
    /// Runs finished.
    pub runs: u64,
    /// Runs that failed and left the job to retry.
    pub retries: u64,
    /// Runs that left the job dead.
    pub dead: u64,
    /// The mean run.
    pub mean: Duration,
}

/// One job kind's queue right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindQueue {
    /// The kind's name.
    pub kind: &'static str,
    /// Due and waiting.
    pub ready: u64,
    /// Waiting for a later time.
    pub scheduled: u64,
    /// Running now.
    pub running: u64,
    /// Dead.
    pub dead: u64,
    /// The oldest ready job's wait.
    pub oldest_ready: Duration,
}

/// Failed actions of one kind and reason across servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailedReason {
    /// The action kind.
    pub kind: ActionKind,
    /// Why they failed.
    pub reason: ActionFailure,
    /// How many.
    pub count: u64,
    /// In how many servers.
    pub servers: u64,
}

/// What `/ops metrics jobs` shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricsJobs {
    /// Runs by kind (kinds that ran).
    pub runs: Vec<JobRuns>,
    /// How far back the runs go (24 hours, or since start).
    pub runs_cover: Duration,
    /// The queue by kind (kinds with rows).
    pub queue: Vec<KindQueue>,
    /// Failed actions by kind and reason, largest first.
    pub failed: Vec<FailedReason>,
    /// When this was read.
    pub updated_at: OffsetDateTime,
}

/// "85ms", "2.4s", "3m": short enough for a table column.
fn short(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}.{}s", ms / 1_000, ms % 1_000 / 100)
    } else {
        format!("{}m", ms / 60_000)
    }
}

/// The most failed-action lines `ops.metrics_jobs` lists.
const FAILED_SHOWN: usize = 8;

fn runs_section(msg: Msg, runs: &[JobRuns]) -> Msg {
    let msg = msg.block(Block::Text(Text::new().bold("Runs by kind")));
    if runs.is_empty() {
        return msg.para(Text::fixed("No runs yet."));
    }
    let mut rows = vec![vec![
        "kind".to_owned(),
        "runs".to_owned(),
        "retry".to_owned(),
        "dead".to_owned(),
        "mean".to_owned(),
    ]];
    rows.extend(runs.iter().map(|r| {
        vec![
            r.kind.to_owned(),
            cell(r.runs),
            cell(r.retries),
            cell(r.dead),
            short(r.mean),
        ]
    }));
    msg.block(Block::Code(table(&rows)))
}

fn queue_section(msg: Msg, queue: &[KindQueue]) -> Msg {
    let msg = msg.block(Block::Text(Text::new().bold("Queue now")));
    let busy: Vec<&KindQueue> = queue
        .iter()
        .filter(|q| q.ready + q.scheduled + q.running + q.dead > 0)
        .collect();
    if busy.is_empty() {
        return msg.para(Text::fixed("🟢 Nothing queued."));
    }
    let mut rows = vec![vec![
        "kind".to_owned(),
        "ready".to_owned(),
        "later".to_owned(),
        "run".to_owned(),
        "dead".to_owned(),
    ]];
    rows.extend(busy.iter().map(|q| {
        vec![
            q.kind.to_owned(),
            cell(q.ready),
            cell(q.scheduled),
            cell(q.running),
            cell(q.dead),
        ]
    }));
    let mut msg = msg.block(Block::Code(table(&rows)));
    if let Some(slowest) = busy
        .iter()
        .filter(|q| q.ready > 0)
        .max_by_key(|q| q.oldest_ready)
    {
        msg = msg.para(
            Text::fixed("Oldest ready job: ")
                .push(Inline::Code(slowest.kind.to_owned()))
                .s(format!(
                    ", waiting {}.",
                    fmt::duration(slowest.oldest_ready.as_secs().max(1))
                )),
        );
    }
    msg
}

fn failed_section(msg: Msg, failed: &[FailedReason]) -> Msg {
    let msg = msg.block(Block::Text(
        Text::new().bold("Failed actions waiting for staff"),
    ));
    if failed.is_empty() {
        return msg.para(Text::fixed("🟢 None."));
    }
    let mut lines = Text::new();
    for (i, f) in failed.iter().take(FAILED_SHOWN).enumerate() {
        if i > 0 {
            lines = lines.s("\n");
        }
        lines = lines
            .s(format!(
                "{} × {} ",
                fmt::count(f.count),
                word::action(f.kind)
            ))
            .push(Inline::Code(f.reason.as_str().to_owned()))
            .s(format!(
                " in {}",
                fmt::plural(f.servers, "server", "servers")
            ));
    }
    let mut msg = msg.para(lines);
    let hidden = failed.len().saturating_sub(FAILED_SHOWN);
    if hidden > 0 {
        msg = msg.block(Block::Subtext(Text::fixed(format!(
            "And {} more {}.",
            fmt::count(u64::try_from(hidden).unwrap_or(0)),
            if hidden == 1 { "kind" } else { "kinds" }
        ))));
    }
    msg
}

/// `ops.metrics_jobs`: job runs, the queue, and failed actions.
pub fn metrics_jobs(m: &MetricsJobs) -> Msg {
    let runs: u64 = m.runs.iter().map(|r| r.runs).sum();
    let failures: u64 = m.runs.iter().map(|r| r.retries + r.dead).sum();
    let window = if m.runs_cover.as_secs() >= 86_400 - 3_600 {
        "the last 24 hours".to_owned()
    } else {
        format!(
            "the {} since this run started",
            fmt::duration(m.runs_cover.as_secs().max(1))
        )
    };
    let tone = if m.queue.iter().any(|q| q.dead > 0) {
        Tone::Danger
    } else if failures > 0 || !m.failed.is_empty() {
        Tone::Warning
    } else {
        Tone::Success
    };
    let msg = Msg::new(tone)
        .block(Block::Heading(Text::fixed("⚙️ Jobs")))
        .para(
            Text::new()
                .bold(fmt::count(runs))
                .s(format!(
                    " {} in {window}, ",
                    if runs == 1 { "run" } else { "runs" }
                ))
                .bold(fmt::count(failures))
                .s(" failed"),
        )
        .block(Block::Divider);
    let msg = runs_section(msg, &m.runs);
    let msg = queue_section(msg, &m.queue);
    failed_section(msg, &m.failed).block(Block::Subtext(
        Text::fixed("Runs count this process only; Grafana has the history. Read ")
            .time(m.updated_at, TimeStyle::Relative)
            .s("."),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_durations() {
        assert_eq!(short(Duration::from_millis(85)), "85ms");
        assert_eq!(short(Duration::from_millis(2_450)), "2.4s");
        assert_eq!(short(Duration::from_secs(185)), "3m");
    }

    #[test]
    fn percents_round() {
        assert_eq!(percent(1, 3), "33%");
        assert_eq!(percent(2, 3), "67%");
        assert_eq!(percent(0, 0), "0%");
    }
}
