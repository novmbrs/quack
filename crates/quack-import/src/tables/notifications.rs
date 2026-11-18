//! `case_notifications` → `dms` of kind `case` (§19.4, §19.9). The state
//! mapping is shared with appeal update DMs.

use time::OffsetDateTime;

use crate::ImportError;
use crate::convert::has_class;
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};
use crate::target::{self, DmState, NewDm};

/// A Go notification row's facts.
#[derive(Clone, Debug)]
pub(crate) struct Notice<'a> {
    pub status: &'a str,
    pub code: &'a str,
    pub channel: Option<i64>,
    pub message: Option<i64>,
    pub sent_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

/// What the importer decided, with the §19.9 row or derived value to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decided {
    pub state: DmState,
    pub in_flight: Option<&'static str>,
    pub derived: Option<&'static str>,
}

/// Maps a Go notification. `voided`: the case was voided, so a DM that was
/// never sent is not needed any more.
pub(crate) fn decide(n: &Notice<'_>, voided: bool) -> Decided {
    let at = n.updated_at;
    let plain = |state| Decided {
        state,
        in_flight: None,
        derived: None,
    };
    match n.status {
        "pending" | "prepared" | "claimed" if voided => Decided {
            state: DmState::NotNeeded { at },
            in_flight: Some("dm_pending_on_voided_case_not_needed"),
            derived: None,
        },
        "pending" | "prepared" | "claimed" => Decided {
            state: DmState::Pending,
            in_flight: Some("dm_pending_resumed"),
            derived: None,
        },
        "sending" => Decided {
            state: DmState::Unknown { at },
            in_flight: Some("dm_sending_unknown"),
            derived: None,
        },
        // Go didn't always keep the DM channel; the message ID is enough
        // to know the DM was sent.
        "sent" => match n.message {
            Some(message) => plain(DmState::Sent {
                channel: n.channel,
                message,
                at: n.sent_at.unwrap_or(at),
            }),
            None => Decided {
                state: DmState::Unknown {
                    at: n.sent_at.unwrap_or(at),
                },
                in_flight: None,
                derived: Some("unconfirmed_dm"),
            },
        },
        "failed" if n.code == "case_voided" => plain(DmState::NotNeeded { at }),
        "failed" if n.code == "delivery_outcome_unknown" => plain(DmState::Unknown { at }),
        "failed" if n.code == "delivery_deferred" => Decided {
            state: DmState::Pending,
            in_flight: Some("dm_pending_resumed"),
            derived: None,
        },
        "failed" => plain(DmState::Failed {
            failure: failure(n.code),
            at,
        }),
        _ => Decided {
            state: DmState::Unknown { at },
            in_flight: None,
            derived: Some("unknown_dm_status"),
        },
    }
}

/// A Go DM failure code → `dm_failure`.
pub(crate) fn failure(code: &str) -> &'static str {
    if has_class(code, "permission_or_hierarchy_denied") {
        "dms_closed"
    } else if code == "dm_prepare_failed" {
        "discord_unavailable"
    } else {
        "discord_refused"
    }
}

/// Imports the server's case DMs.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    const SQL: &str = "SELECT n.* FROM case_notifications n JOIN cases c ON c.id = n.case_id
                       WHERE c.guild_id = ? AND n.id > ? ORDER BY n.id";
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        let rows = cx.src.page(SQL, &[&g], &after).await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "case_notifications", "id").key().to_owned();
        for row in &rows {
            one(cx, &Cols::new(row, "case_notifications", "id")).await?;
        }
        if rows.len() < BATCH {
            break;
        }
    }
    Ok(())
}

async fn one(cx: &mut Ctx<'_>, c: &Cols<'_>) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let (Some(id), Some(case_id)) = (c.uuid("id")?, c.uuid("case_id")?) else {
        cx.f.skip("case_notifications", "invalid_id", &key);
        return Ok(());
    };
    let Some(case) = cx.cases.get(&case_id).cloned() else {
        cx.f.skip("case_notifications", "missing_parent", &key);
        return Ok(());
    };
    let decided = decide(
        &Notice {
            status: &c.text("status")?,
            code: &c.text("last_error_code")?,
            channel: c.snowflake("prepared_channel_discord_id")?.id(),
            message: c.snowflake("delivery_message_discord_id")?.id(),
            sent_at: c.opt_time("sent_at")?,
            updated_at: c.time("updated_at")?,
        },
        case.voided,
    );
    record(cx, &key, &decided);
    let dm = NewDm {
        id,
        member_id: case.member,
        kind: "case",
        case_id: Some(case_id),
        appeal_event_id: None,
        ticket_id: None,
        state: decided.state,
        created_at: c.time("created_at")?,
    };
    target::dm(&mut cx.tx, &mut cx.f, cx.g.id, &key, &dm).await?;
    Ok(())
}

/// Reports a decision's §19.9 row and derived value.
pub(crate) fn record(cx: &mut Ctx<'_>, key: &str, d: &Decided) {
    if let Some(row) = d.in_flight {
        cx.f.in_flight(row, key);
    }
    if let Some(code) = d.derived {
        cx.f.derived(code, key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(
        status: &'static str,
        code: &'static str,
        channel: Option<i64>,
        message: Option<i64>,
    ) -> Notice<'static> {
        Notice {
            status,
            code,
            channel,
            message,
            sent_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn states() {
        let at = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            decide(&n("pending", "", None, None), false).state,
            DmState::Pending
        );
        assert_eq!(
            decide(&n("claimed", "", None, None), true).state,
            DmState::NotNeeded { at }
        );
        assert_eq!(
            decide(&n("sending", "", Some(1), None), false).state,
            DmState::Unknown { at }
        );
        assert_eq!(
            decide(&n("sent", "", Some(1), Some(2)), false).state,
            DmState::Sent {
                channel: Some(1),
                message: 2,
                at
            }
        );
        assert_eq!(
            decide(&n("sent", "", None, Some(2)), false).state,
            DmState::Sent {
                channel: None,
                message: 2,
                at
            }
        );
        let d = decide(&n("sent", "", Some(1), None), false);
        assert_eq!(
            (d.state, d.derived),
            (DmState::Unknown { at }, Some("unconfirmed_dm"))
        );
        assert_eq!(
            decide(&n("failed", "case_voided", None, None), false).state,
            DmState::NotNeeded { at }
        );
        assert_eq!(
            decide(
                &n("failed", "dm_permission_or_hierarchy_denied", None, None),
                false
            )
            .state,
            DmState::Failed {
                failure: "dms_closed",
                at
            }
        );
        assert_eq!(
            decide(&n("failed", "dm_prepare_failed", None, None), false).state,
            DmState::Failed {
                failure: "discord_unavailable",
                at
            }
        );
        assert_eq!(
            decide(&n("failed", "other", None, None), false).state,
            DmState::Failed {
                failure: "discord_refused",
                at
            }
        );
        assert_eq!(
            decide(&n("failed", "delivery_outcome_unknown", None, None), false).state,
            DmState::Unknown { at }
        );
        assert_eq!(
            decide(&n("failed", "delivery_deferred", None, None), false).state,
            DmState::Pending
        );
    }

    #[test]
    fn a_dm_that_may_have_been_sent_is_never_pending() {
        for status in ["sending", "sent", "failed", "weird"] {
            for code in ["", "delivery_outcome_unknown", "case_voided"] {
                assert_ne!(
                    decide(&n(status, code, Some(1), None), false).state,
                    DmState::Pending,
                    "{status} {code}"
                );
            }
        }
    }
}
