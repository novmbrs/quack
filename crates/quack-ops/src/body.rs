//! What a durable ops post says (`ops_posts.body`): typed facts saved as
//! JSON, rendered by `quack_text::ops` when the post is sent, so a post
//! waiting through a restart still renders with the current wording.

use quack_core::action::{ActionFailure, ActionKind};
use quack_core::ids::{AccountId, GuildId, JobId};
use quack_db::jobs::JobKind;
use quack_text::Msg;
use quack_text::ops::{self as text, InstallChange};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// One durable post.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Body {
    /// `ops.started`.
    Started {
        version: String,
        commit: Option<String>,
        #[serde(with = "time::serde::rfc3339")]
        started_at: OffsetDateTime,
        ready_in_seconds: u64,
        migrations_applied: usize,
        servers: u64,
        previous: Previous,
    },
    /// `ops.gateway`.
    Gateway {
        drops: u64,
        connect_failures: u64,
        identifies: u64,
        resumes: u64,
        invalid_sessions: u64,
        connected_now: bool,
    },
    /// `ops.install`. The fields after `servers` came later, so a post
    /// saved before them still reads (as unknown).
    Install {
        change: Change,
        name: String,
        guild_id: GuildId,
        owner_id: AccountId,
        servers: u64,
        #[serde(default)]
        members: Option<u64>,
        #[serde(default)]
        total_members: Option<u64>,
        #[serde(default, with = "time::serde::rfc3339::option")]
        since: Option<OffsetDateTime>,
        #[serde(default, with = "time::serde::rfc3339::option")]
        left_at: Option<OffsetDateTime>,
        #[serde(default)]
        cases: Option<u64>,
        #[serde(default)]
        rules: Option<u64>,
    },
    /// `ops.dead_job`.
    DeadJob(DeadJob),
    /// `ops.dead_jobs_more`.
    DeadJobsMore { more: u64 },
    /// `ops.failed_actions`: the largest groups, with every failure and
    /// group counted.
    FailedActions {
        groups: Vec<FailedGroup>,
        total: u64,
        hidden_groups: u64,
    },
}

/// The run before, as saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Previous {
    None,
    Stopped {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        gateway_closed: bool,
    },
    Ended {
        #[serde(with = "time::serde::rfc3339")]
        seen_at: OffsetDateTime,
    },
}

/// An install change, as saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Change {
    Joined,
    Rejoined,
    Left,
}

/// A dead job, as saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeadJob {
    pub(crate) job_id: JobId,
    pub(crate) job_kind: String,
    pub(crate) key: String,
    pub(crate) guild_id: Option<GuildId>,
    pub(crate) attempts: u32,
    pub(crate) error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) dead_at: OffsetDateTime,
}

impl From<&quack_db::ops::DeadJob> for DeadJob {
    fn from(j: &quack_db::ops::DeadJob) -> Self {
        Self {
            job_id: j.id,
            job_kind: j.kind.as_str().to_owned(),
            key: j.key.clone(),
            guild_id: j.guild,
            attempts: j.attempts,
            error: j.last_error.clone(),
            dead_at: j.dead_at,
        }
    }
}

impl DeadJob {
    /// The view `quack_text::ops` renders.
    pub(crate) fn view(&self) -> text::DeadJob {
        text::DeadJob {
            id: self.job_id,
            kind: job_kind_name(&self.job_kind),
            key: self.key.clone(),
            guild: self.guild_id,
            attempts: self.attempts,
            error: self.error.clone(),
            dead_at: self.dead_at,
        }
    }
}

/// A job kind's static name; `unknown` for one this build doesn't know.
pub(crate) fn job_kind_name(kind: &str) -> &'static str {
    kind.parse::<JobKind>().map_or("unknown", JobKind::as_str)
}

/// Failed actions of one kind and reason in one server, as saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailedGroup {
    pub(crate) guild_id: GuildId,
    pub(crate) action: ActionKind,
    pub(crate) reason: ActionFailure,
    pub(crate) count: u64,
}

impl Body {
    /// The message to post.
    pub(crate) fn render(&self) -> Msg {
        match self {
            Self::Started {
                version,
                commit,
                started_at,
                ready_in_seconds,
                migrations_applied,
                servers,
                previous,
            } => text::started(&text::Started {
                version: version.clone(),
                commit: commit.clone(),
                started_at: *started_at,
                ready_in_seconds: *ready_in_seconds,
                migrations_applied: *migrations_applied,
                servers: *servers,
                previous: match *previous {
                    Previous::None => text::Previous::None,
                    Previous::Stopped { at, gateway_closed } => text::Previous::Stopped {
                        at,
                        reason: if gateway_closed {
                            text::StopReason::GatewayClosed
                        } else {
                            text::StopReason::Signal
                        },
                    },
                    Previous::Ended { seen_at } => text::Previous::Ended { seen_at },
                },
            }),
            Self::Gateway {
                drops,
                connect_failures,
                identifies,
                resumes,
                invalid_sessions,
                connected_now,
            } => text::gateway(&text::Gateway {
                drops: *drops,
                connect_failures: *connect_failures,
                identifies: *identifies,
                resumes: *resumes,
                invalid_sessions: *invalid_sessions,
                connected_now: *connected_now,
            }),
            Self::Install {
                change,
                name,
                guild_id,
                owner_id,
                servers,
                members,
                total_members,
                since,
                left_at,
                cases,
                rules,
            } => text::install(&text::Install {
                change: match change {
                    Change::Joined => InstallChange::Joined,
                    Change::Rejoined => InstallChange::Rejoined,
                    Change::Left => InstallChange::Left,
                },
                name: name.clone(),
                guild: *guild_id,
                owner: *owner_id,
                servers: *servers,
                members: *members,
                total_members: *total_members,
                since: *since,
                left_at: *left_at,
                cases: *cases,
                rules: *rules,
            }),
            Self::DeadJob(job) => text::dead_job(&job.view()),
            Self::DeadJobsMore { more } => text::dead_jobs_more(*more),
            Self::FailedActions {
                groups,
                total,
                hidden_groups,
            } => text::failed_actions(
                &groups
                    .iter()
                    .map(|g| text::FailedGroup {
                        guild: g.guild_id,
                        kind: g.action,
                        reason: g.reason,
                        count: g.count,
                    })
                    .collect::<Vec<_>>(),
                *total,
                *hidden_groups,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_round_trip_through_json() {
        let at = time::macros::datetime!(2026-11-20 10:00 UTC);
        let bodies = [
            Body::Started {
                version: "0.1.0".to_owned(),
                commit: None,
                started_at: at,
                ready_in_seconds: 3,
                migrations_applied: 0,
                servers: 2,
                previous: Previous::Ended { seen_at: at },
            },
            Body::FailedActions {
                groups: vec![FailedGroup {
                    guild_id: GuildId::new(5).unwrap(),
                    action: ActionKind::Ban,
                    reason: ActionFailure::QuackRoleTooLow,
                    count: 2,
                }],
                total: 2,
                hidden_groups: 0,
            },
            Body::DeadJobsMore { more: 3 },
            Body::Install {
                change: Change::Left,
                name: "Pond".to_owned(),
                guild_id: GuildId::new(5).unwrap(),
                owner_id: AccountId::new(6).unwrap(),
                servers: 2,
                members: Some(40),
                total_members: Some(900),
                since: Some(at),
                left_at: Some(at),
                cases: Some(3),
                rules: Some(1),
            },
        ];
        for body in bodies {
            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(serde_json::from_value::<Body>(json).unwrap(), body);
        }
    }

    #[test]
    fn an_install_saved_before_member_counts_still_reads() {
        let old = serde_json::json!({
            "kind": "install",
            "change": "joined",
            "name": "Pond",
            "guild_id": "5",
            "owner_id": "6",
            "servers": 2
        });
        let body: Body = serde_json::from_value(old).unwrap();
        let text = body.render().plain();
        assert!(text.contains("Quack is in 2 servers now."), "{text}");
        assert!(!text.contains("Members"), "{text}");
    }

    #[test]
    fn failed_actions_keep_the_whole_count() {
        let body = Body::FailedActions {
            groups: vec![FailedGroup {
                guild_id: GuildId::new(5).unwrap(),
                action: ActionKind::Ban,
                reason: ActionFailure::QuackRoleTooLow,
                count: 2,
            }],
            total: 30,
            hidden_groups: 15,
        };
        let text = body.render().plain();
        assert!(text.contains("30 actions failed"), "{text}");
        assert!(text.contains("And 15 more groups."), "{text}");
    }

    #[test]
    fn an_unknown_job_kind_still_renders() {
        assert_eq!(job_kind_name("post_ops"), "post_ops");
        assert_eq!(job_kind_name("gone_kind"), "unknown");
    }
}
