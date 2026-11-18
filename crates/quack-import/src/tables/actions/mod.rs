//! `case_action_executions` → `actions`, `case_action_attempts` →
//! `action_attempts` (§19.7, §19.9). Originals first, then undos, so an
//! undo's original exists; each batch's attempts are read with it.

mod attempts;
mod state;

use std::collections::{HashMap, HashSet};

use quack_core::action::{ActionKind, AttemptOutcome};
use quack_core::ids::ActionId;
use quack_core::limits;
use quack_db::jobs::NewJob;
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use self::attempts::{Attempt, attempts_of, write_attempts};
use self::state::{Facts, State};
use crate::ImportError;
use crate::guild::{ActionInfo, CaseInfo, Ctx};
use crate::source::{Cols, Pages};
use crate::target::{self, strict};

const SELECT: &str = "SELECT e.* FROM case_action_executions e JOIN cases c ON c.id = e.case_id
                      WHERE c.guild_id = ? AND e.reversal_of_execution_id IS {} NULL AND e.id > ? ORDER BY e.id";

/// Imports the server's actions and their attempts.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let losers = duplicates(cx).await?;
    for undo in [false, true] {
        let sql = SELECT.replace("{}", if undo { "NOT" } else { "" });
        let g = cx.g.ulid.clone();
        let mut pages = Pages::new(&sql, "case_action_executions", "id");
        while let Some(rows) = pages.next(cx.src, &[&g]).await? {
            let mut attempts = attempts_of(cx, &rows).await?;
            for row in &rows {
                let c = Cols::new(row, "case_action_executions", "id");
                let tries = attempts.remove(c.key()).unwrap_or_default();
                if losers.contains(c.key()) {
                    let key = cx.key(c.key());
                    cx.f.skip(
                        "case_action_executions",
                        if undo {
                            "duplicate_undo"
                        } else {
                            "duplicate_original"
                        },
                        &key,
                    );
                    continue;
                }
                execution(cx, &c, undo, tries).await?;
            }
        }
    }
    Ok(())
}

/// Executions that cannot be kept because the new schema allows one
/// original per case and one undo per original: the earliest original and
/// the latest undo (the current state) are kept.
async fn duplicates(cx: &mut Ctx<'_>) -> Result<HashSet<String>, ImportError> {
    let g = cx.g.ulid.clone();
    let rows = cx
        .src
        .rows(
            "SELECT d.id, d.slot, d.is_undo FROM (
               SELECT e.id, e.created_at, COALESCE(e.reversal_of_execution_id, e.case_id) AS slot,
                      CAST(e.reversal_of_execution_id IS NOT NULL AS SIGNED) AS is_undo
               FROM case_action_executions e JOIN cases c ON c.id = e.case_id
               WHERE c.guild_id = ? AND e.action_type <> 'send_dm') d
             JOIN (
               SELECT COALESCE(x.reversal_of_execution_id, x.case_id) AS slot,
                      CAST(x.reversal_of_execution_id IS NOT NULL AS SIGNED) AS is_undo
               FROM case_action_executions x JOIN cases y ON y.id = x.case_id
               WHERE y.guild_id = ? AND x.action_type <> 'send_dm'
               GROUP BY 1, 2 HAVING COUNT(*) > 1) dup ON dup.slot = d.slot AND dup.is_undo = d.is_undo
             ORDER BY d.slot, d.created_at, d.id",
            &[&g, &g],
        )
        .await?;
    let mut by_slot: HashMap<(String, bool), Vec<String>> = HashMap::new();
    for row in &rows {
        let c = Cols::new(row, "case_action_executions", "id");
        let is_undo = c.int("is_undo")? != 0;
        by_slot
            .entry((c.text("slot")?, is_undo))
            .or_default()
            .push(c.key().to_owned());
    }
    let mut losers = HashSet::new();
    for ((_, is_undo), mut ids) in by_slot {
        if ids.len() < 2 {
            continue;
        }
        if is_undo {
            ids.pop();
        } else {
            ids.remove(0);
        }
        losers.extend(ids);
    }
    Ok(losers)
}

/// Where an execution goes: its action ID, kind, case, and original.
struct Placed {
    id: Uuid,
    kind: ActionKind,
    case_id: Uuid,
    case: CaseInfo,
    undo_of: Option<Uuid>,
}

/// Finds the execution's case and original, or reports why it is skipped
/// (with its attempts).
fn place(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    key: &str,
    undo: bool,
    attempts: &[Attempt],
) -> Result<Option<Placed>, ImportError> {
    let action_type = c.text("action_type")?;
    let reason = if action_type == "send_dm" {
        "dm_action_row"
    } else {
        "missing_parent"
    };
    let skip_all = |cx: &mut Ctx<'_>, why: &str| {
        cx.f.skip("case_action_executions", why, key);
        for a in attempts.iter().filter_map(|a| a.number) {
            cx.f.skip("case_action_attempts", reason, format!("{key}:{a}"));
        }
    };
    if action_type == "send_dm" {
        skip_all(cx, "dm_action_row");
        return Ok(None);
    }
    let (Some(id), Some(kind)) = (c.uuid("id")?, state::kind(&action_type)) else {
        skip_all(cx, "unknown_action_type");
        return Ok(None);
    };
    let Some((case_id, case)) = c
        .uuid("case_id")?
        .and_then(|id| cx.cases.get(&id).map(|k| (id, k.clone())))
    else {
        skip_all(cx, "missing_parent");
        return Ok(None);
    };
    if undo != matches!(kind, ActionKind::RemoveTimeout | ActionKind::Unban) {
        skip_all(cx, "invalid_undo");
        return Ok(None);
    }
    let undo_of = match c.uuid("reversal_of_execution_id")? {
        None => None,
        Some(original)
            if cx
                .actions
                .get(&original)
                .is_some_and(|info| info.case == case_id) =>
        {
            Some(original)
        }
        Some(_) => {
            skip_all(cx, "missing_parent");
            return Ok(None);
        }
    };
    Ok(Some(Placed {
        id,
        kind,
        case_id,
        case,
        undo_of,
    }))
}

#[expect(
    clippy::too_many_lines,
    reason = "one action's columns, read top to bottom"
)]
async fn execution(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    undo: bool,
    attempts: Vec<Attempt>,
) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let Some(Placed {
        id,
        kind,
        case_id,
        case,
        undo_of,
    }) = place(cx, c, &key, undo, &attempts)?
    else {
        return Ok(());
    };
    let outcomes: Vec<AttemptOutcome> = attempts.iter().map(|a| a.outcome).collect();
    let last_ok = attempts
        .iter()
        .rev()
        .find(|a| matches!(a.outcome, AttemptOutcome::Done | AttemptOutcome::NoChange));
    let status = c.text("status")?;
    let (state, in_flight) = state::state(&Facts {
        status: &status,
        last_error_code: &c.text("last_error_code")?,
        kind,
        undo,
        case_voided: case.voided,
        attempts: &outcomes,
        no_change: last_ok.is_some_and(|a| a.outcome == AttemptOutcome::NoChange),
    });
    if let Some(row) = in_flight {
        cx.f.in_flight(row, &key);
    }
    let config = c.json("config_snapshot_json")?;
    let seconds = |k: &str| config.get(k).and_then(Value::as_i64);
    let timeout_ends_at = (kind == ActionKind::Timeout).then(|| {
        timeout_end(
            cx,
            &key,
            state,
            last_ok,
            case.created_at,
            seconds("duration_seconds").or(case.timeout_seconds.map(i64::from)),
        )
    });
    let delete = (kind == ActionKind::Ban).then(|| {
        seconds("delete_message_seconds")
            .and_then(|s| i32::try_from(s).ok())
            .or(case.ban_delete_seconds)
            .unwrap_or(0)
    });
    let requested_by =
        crate::convert::snowflake(config.get("requested_by").and_then(Value::as_str)).id();
    let (label, failure, http, no_change, skip, done_at) = match state {
        State::Pending => ("pending", None, None, false, None, None),
        State::Done { no_change } => (
            "done",
            None,
            None,
            no_change,
            None,
            Some(
                c.opt_time("finished_at")?
                    .map_or_else(|| c.time("updated_at"), Ok)?,
            ),
        ),
        State::Cancelled => ("cancelled", None, None, false, None, None),
        State::Failed { reason, http } => {
            ("failed", Some(reason.as_str()), http, false, None, None)
        }
        State::Skipped => (
            "skipped",
            None,
            None,
            false,
            Some("changed_outside_case"),
            None,
        ),
    };
    // Only a failed action can be dismissed; Go didn't always record who.
    let dismissed = match (
        c.opt_time("dismissed_at")?,
        c.snowflake("dismissed_by_discord_user_id")?.id(),
    ) {
        (None, _) => (None, None),
        (Some(at), by) if label == "failed" => {
            if by.is_none() {
                cx.f.derived("dismissed_by_unknown", &key);
            }
            (Some(at), by)
        }
        _ => {
            cx.f.derived("dismissal_dropped", &key);
            (None, None)
        }
    };
    let q = sqlx::query(
        "INSERT INTO actions (id, guild_id, case_id, member_id, kind, undo_of_action_id, state, timeout_ends_at,
                              delete_message_seconds, requested_by, attempts, last_attempt_at, failure_reason,
                              failure_http_status, skip_reason, no_change, done_at, dismissed_at, dismissed_by,
                              created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5::action_kind, $6, $7::action_state, $8, $9, $10, 0, $11, $12::action_failure, $13,
                 $14::action_skip_reason, $15, $16, $17, $18, $19, $20)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(case_id)
    .bind(case.member)
    .bind(kind.as_str())
    .bind(undo_of)
    .bind(label)
    .bind(timeout_ends_at)
    .bind(delete)
    .bind(requested_by)
    .bind(attempts.iter().map(|a| a.started_at).max())
    .bind(failure)
    .bind(http)
    .bind(skip)
    .bind(no_change)
    .bind(done_at)
    .bind(dismissed.0)
    .bind(dismissed.1)
    .bind(c.time("created_at")?)
    .bind(c.time("updated_at")?);
    strict(&mut cx.tx, &mut cx.f, "actions", &key, q).await?;
    cx.actions.insert(id, ActionInfo { case: case_id });
    write_attempts(cx, id, &key, &attempts).await?;
    if state == State::Pending {
        let job = NewJob::run_action(target::guild(cx.g.id)?, ActionId::from_uuid(id));
        target::job(&mut cx.tx, &mut cx.f, job).await?;
    }
    Ok(())
}

/// A timeout's end (§19.7): a done timeout's `timeout_until` from Discord's
/// answer; otherwise (or when missing) `case.created_at + duration`, in
/// whole seconds.
fn timeout_end(
    cx: &mut Ctx<'_>,
    key: &str,
    state: State,
    last_ok: Option<&Attempt>,
    case_created: OffsetDateTime,
    duration: Option<i64>,
) -> OffsetDateTime {
    if matches!(state, State::Done { .. }) {
        let answered = last_ok
            .and_then(|a| a.response.get("timeout_until"))
            .and_then(Value::as_str)
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            });
        if let Some(at) = answered {
            return at;
        }
        cx.f.derived("derived_timeout_end", key);
    }
    let Some(end) = end_after(case_created, duration) else {
        cx.f.derived("timeout_without_duration", key);
        return case_created.replace_nanosecond(0).unwrap_or(case_created);
    };
    end.replace_nanosecond(0).unwrap_or(end)
}

/// `created + duration` when the duration is within Discord's timeout
/// bounds. Otherwise `None`: there is no safe end, and the caller uses the
/// case's creation (already past), so it can't time anyone out.
fn end_after(created: OffsetDateTime, duration: Option<i64>) -> Option<OffsetDateTime> {
    let bounds = i64::from(limits::MIN_TIMEOUT_SECONDS)..=i64::from(limits::MAX_TIMEOUT_SECONDS);
    duration
        .filter(|d| bounds.contains(d))
        .and_then(|d| created.checked_add(time::Duration::seconds(d)))
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    #[test]
    fn an_out_of_range_timeout_duration_has_no_end_and_never_panics() {
        let created = datetime!(2025-01-01 00:00 UTC);
        assert_eq!(
            super::end_after(created, Some(60)),
            Some(datetime!(2025-01-01 00:01 UTC))
        );
        for bad in [
            None,
            Some(0),
            Some(-5),
            Some(2_419_201),
            Some(i64::MAX),
            Some(i64::MIN),
        ] {
            assert_eq!(super::end_after(created, bad), None, "{bad:?}");
        }
    }
}
