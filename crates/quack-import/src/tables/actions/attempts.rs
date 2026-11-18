//! `case_action_attempts` → `action_attempts`: reading a batch's attempts
//! and writing an action's.

use std::collections::HashMap;

use quack_core::action::AttemptOutcome;
use serde_json::Value;
use sqlx::mysql::MySqlRow;
use time::OffsetDateTime;
use uuid::Uuid;

use super::state;
use crate::ImportError;
use crate::guild::Ctx;
use crate::source::Cols;
use crate::target::guarded;

/// One attempt as read.
pub(super) struct Attempt {
    pub(super) number: i32,
    pub(super) outcome: AttemptOutcome,
    pub(super) started_at: OffsetDateTime,
    pub(super) finished_at: OffsetDateTime,
    pub(super) response: Value,
}

pub(super) async fn attempts_of(
    cx: &mut Ctx<'_>,
    rows: &[MySqlRow],
) -> Result<HashMap<String, Vec<Attempt>>, ImportError> {
    let ids: Vec<String> = rows
        .iter()
        .map(|r| {
            Cols::new(r, "case_action_executions", "id")
                .key()
                .to_owned()
        })
        .collect();
    let placeholders = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT * FROM case_action_attempts WHERE execution_id IN ({placeholders}) ORDER BY execution_id, attempt_number"
    );
    let binds: Vec<&str> = ids.iter().map(String::as_str).collect();
    let found = cx.src.rows(&sql, &binds).await?;
    let mut out: HashMap<String, Vec<Attempt>> = HashMap::new();
    for row in &found {
        let c = Cols::new(row, "case_action_attempts", "id");
        let key = cx.key(c.key());
        let number = i32::from(u8::try_from(c.unsigned("attempt_number")?).unwrap_or(0));
        if number < 1 {
            cx.f.skip("case_action_attempts", "invalid_attempt_number", &key);
            continue;
        }
        let status = c.text("status")?;
        let response = c.json("response_payload_json")?;
        let outcome = state::attempt_outcome(&status, &c.text("error_code")?, &response);
        let started_at = c.time("started_at")?;
        let finished_at = match c.opt_time("finished_at")? {
            Some(at) if at < started_at => {
                cx.f.normalized("action_attempts.finished_at", &key);
                started_at
            }
            Some(at) => at,
            None => {
                cx.f.in_flight("open_attempt_interrupted", &key);
                started_at
            }
        };
        let execution = c.text("execution_id")?;
        out.entry(execution).or_default().push(Attempt {
            number,
            outcome,
            started_at,
            finished_at,
            response,
        });
    }
    Ok(out)
}

pub(super) async fn write_attempts(
    cx: &mut Ctx<'_>,
    id: Uuid,
    key: &str,
    attempts: &[Attempt],
) -> Result<(), ImportError> {
    for a in attempts {
        let q = sqlx::query(
            "INSERT INTO action_attempts (guild_id, action_id, number, started_at, finished_at, outcome)
             VALUES ($1, $2, $3, $4, $5, $6::attempt_outcome)",
        )
        .bind(cx.g.id)
        .bind(id)
        .bind(a.number)
        .bind(a.started_at)
        .bind(a.finished_at)
        .bind(a.outcome.as_str());
        guarded(
            &mut cx.tx,
            &mut cx.f,
            "action_attempts",
            &format!("{key}:{}", a.number),
            q,
        )
        .await?;
    }
    Ok(())
}
