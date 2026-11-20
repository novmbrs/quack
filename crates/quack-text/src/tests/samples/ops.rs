//! §12.15 ops messages.

use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::{GuildId, JobId};
use uuid::Uuid;

use super::{Out, Sample, label, msg, sample, text};
use crate::budget::COMMAND_DESCRIPTION;
use crate::command;
use crate::ops::{
    self, Backlog, DeadJob, ErrorGroup, FailedGroup, Gateway, Handled, Install, InstallChange,
    Previous, Server, Started, Status, StopReason,
};
use crate::tests::fixtures::*;

fn guild() -> GuildId {
    GuildId::new(1_005_778_938_108_325_970).unwrap()
}

fn job(n: u128) -> DeadJob {
    DeadJob {
        id: JobId::from_uuid(Uuid::from_u128(
            0x0193_0000_0000_7000_8000_0000_0000_0000 + n,
        )),
        kind: "run_action",
        key: format!("run_action:0193-{n}"),
        guild: Some(guild()),
        attempts: 8,
        error: Some("discord_unavailable".to_owned()),
        dead_at: t0(),
    }
}

fn started(previous: Previous, commit: Option<&str>) -> Started {
    Started {
        version: "0.1.0".to_owned(),
        commit: commit.map(str::to_owned),
        started_at: later(),
        ready_in_seconds: 7,
        migrations_applied: 1,
        servers: 1_204,
        previous,
    }
}

fn status() -> Status {
    Status {
        version: "0.1.0".to_owned(),
        started_at: t0(),
        gateway_connected: true,
        latency_ms: Some(42),
        servers: 1_204,
        backlog: vec![Backlog {
            kind: "run_action",
            ready: 3,
            oldest_seconds: 12,
        }],
        dead_jobs: 0,
        failed_actions: 2,
        open_appeals: 5,
        open_tickets: 1,
        pool_used: 3,
        pool_size: 20,
        updated_at: later(),
    }
}

fn server(installed: bool) -> Server {
    Server {
        guild: guild(),
        name: "Quack Pond".to_owned(),
        owner: account(444),
        installed,
        first_joined_at: t0(),
        left_at: (!installed).then(later),
        cases: 120,
        voided_cases: 4,
        active_rules: 6,
        open_appeals: 1,
        failed_actions: 0,
        open_tickets: 2,
        modules_on: vec!["tickets", "honeypot"],
        missing_permissions: Some(if installed {
            Vec::new()
        } else {
            vec!["Ban Members".to_owned()]
        }),
    }
}

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for (id, c) in [
        "command.ops",
        "command.ops.status",
        "command.ops.server",
        "command.ops.server.id",
        "command.ops.jobs",
    ]
    .into_iter()
    .zip(command::OPS_ALL)
    {
        v.push(label(
            id,
            format!("{} — {}", c.name, c.description),
            COMMAND_DESCRIPTION + 40,
        ));
    }
    v.push(msg(
        "ops.started",
        "first",
        ops::started(&started(Previous::None, Some("f128a3e5c0ffee"))),
    ));
    v.push(msg(
        "ops.started",
        "after a deploy",
        ops::started(&started(
            Previous::Stopped {
                at: t0(),
                reason: StopReason::Signal,
            },
            None,
        )),
    ));
    v.push(msg(
        "ops.started",
        "after the gateway closed",
        ops::started(&started(
            Previous::Stopped {
                at: t0(),
                reason: StopReason::GatewayClosed,
            },
            None,
        )),
    ));
    v.push(msg(
        "ops.started",
        "after a crash",
        ops::started(&started(Previous::Ended { seen_at: t0() }, None)),
    ));
    v.push(msg(
        "ops.gateway",
        "connected",
        ops::gateway(&Gateway {
            drops: 2,
            connect_failures: 1,
            identifies: 0,
            resumes: 2,
            invalid_sessions: 0,
            connected_now: true,
        }),
    ));
    v.push(msg(
        "ops.gateway",
        "down",
        ops::gateway(&Gateway {
            drops: 1,
            connect_failures: 5,
            identifies: 1,
            resumes: 0,
            invalid_sessions: 1,
            connected_now: false,
        }),
    ));
    for (variant, change) in [
        ("joined", InstallChange::Joined),
        ("rejoined", InstallChange::Rejoined),
        ("left", InstallChange::Left),
    ] {
        v.push(msg(
            "ops.install",
            variant,
            ops::install(&Install {
                change,
                name: "Quack *Pond*".to_owned(),
                guild: guild(),
                owner: account(444),
                servers: 1_205,
            }),
        ));
    }
    v.push(msg("ops.dead_job", "", ops::dead_job(&job(1))));
    v.push(msg(
        "ops.dead_job",
        "no error, no server",
        ops::dead_job(&DeadJob {
            guild: None,
            error: None,
            kind: "housekeeping",
            key: "housekeeping".to_owned(),
            attempts: 1,
            ..job(2)
        }),
    ));
    v.push(msg(
        "ops.dead_job_handled",
        "requeued",
        ops::dead_job_handled(&job(1), Handled::Requeued, account(444)),
    ));
    v.push(msg(
        "ops.dead_job_handled",
        "dismissed",
        ops::dead_job_handled(&job(1), Handled::Dismissed, account(444)),
    ));
    v.push(msg("ops.dead_jobs_more", "", ops::dead_jobs_more(7)));
    v.push(text("ops.job_gone", "", ops::job_gone()));
    let failed: Vec<FailedGroup> = (0..12)
        .map(|n| FailedGroup {
            guild: GuildId::new(1000 + n).unwrap(),
            kind: if n % 2 == 0 {
                ActionKind::Ban
            } else {
                ActionKind::RemoveTimeout
            },
            reason: if n % 3 == 0 {
                ActionFailure::QuackRoleTooLow
            } else {
                ActionFailure::RetriesExhausted
            },
            count: 13 - n,
        })
        .collect();
    v.push(msg(
        "ops.failed_actions",
        "one",
        ops::failed_actions(&failed[..1], 13, 0),
    ));
    v.push(msg(
        "ops.failed_actions",
        "more than shown",
        ops::failed_actions(&failed[..10], 90 + 40, 2 + 38),
    ));
    let group = |n: u64, url: bool| ErrorGroup {
        target: "quack_engine::actions::run".to_owned(),
        message: format!("couldn't settle an attempt error=database {n}"),
        count: n,
        trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".to_owned()),
        trace_url: url.then(|| {
            "https://grafana.example/explore?trace=4bf92f3577b34da6a3ce929d0e0e4736".to_owned()
        }),
    };
    v.push(msg("ops.errors", "one", ops::errors(&[group(1, false)], 0)));
    v.push(msg(
        "ops.errors",
        "linked, dropped",
        ops::errors(&[group(3, true), group(1, true)], 12),
    ));
    v.push(msg(
        "ops.errors",
        "more than shown",
        ops::errors(&(1..=12).map(|n| group(n, false)).collect::<Vec<_>>(), 0),
    ));
    v.push(msg("ops.status", "board", ops::status(&status(), true)));
    v.push(msg(
        "ops.status",
        "trouble, no button",
        ops::status(
            &Status {
                gateway_connected: false,
                latency_ms: None,
                backlog: Vec::new(),
                dead_jobs: 2,
                ..status()
            },
            false,
        ),
    ));
    v.push(msg("ops.server", "installed", ops::server(&server(true))));
    v.push(msg(
        "ops.server",
        "left, no modules",
        ops::server(&Server {
            modules_on: Vec::new(),
            ..server(false)
        }),
    ));
    v.push(msg(
        "ops.server",
        "discord unavailable",
        ops::server(&Server {
            missing_permissions: None,
            ..server(true)
        }),
    ));
    v.push(text(
        "ops.server_unknown",
        "",
        ops::server_unknown(" 12345 "),
    ));
    v.push(msg("ops.jobs", "none", ops::jobs(&[], 1, 0)));
    v.push(msg(
        "ops.jobs",
        "first of two pages",
        ops::jobs(&(1..=5).map(job).collect::<Vec<_>>(), 1, 7),
    ));
    v.push(msg(
        "ops.jobs",
        "last page",
        ops::jobs(&(6..=7).map(job).collect::<Vec<_>>(), 2, 7),
    ));
    v.push(sample("ops.not_allowed", "", Out::Text(ops::not_allowed())));
    v
}
