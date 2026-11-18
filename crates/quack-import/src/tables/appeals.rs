//! `appeals` → `appeals`, `appeal_events` → `appeal_events` (§19.4),
//! including the legacy `content`/`answers_json` appeal shapes.

use std::collections::HashMap;

use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::{AppealInfo, Ctx};
use crate::source::{BATCH, Cols};
use crate::target::{guarded, strict};

/// What an empty statement, reply, or reason is imported as.
pub(crate) const NO_TEXT: &str = "(no text)";

/// The appeal statement from whichever columns the source has: `statement`,
/// or the legacy `content`; when empty, the legacy form answers joined as Go
/// did (nonblank trimmed strings, blank line between).
pub(crate) fn statement(
    statement: Option<String>,
    content: Option<String>,
    answers: Option<&Value>,
) -> (String, bool) {
    let text = statement.or(content).unwrap_or_default();
    if !text.trim().is_empty() {
        return (text, false);
    }
    let joined = answers
        .and_then(Value::as_array)
        .map_or_else(String::new, |items| {
            items
                .iter()
                .filter_map(|a| a.get("value").and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        });
    (joined, true)
}

/// Go appeal event types → `appeal_event_kind`.
pub(crate) fn event_kind(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "submitted" => "submitted",
        "information_requested" => "information_requested",
        "information_submitted" => "information_provided",
        "reopened" => "reopened",
        "accepted" => "accepted",
        "rejected" => "rejected",
        "closed" => "closed",
        _ => return None,
    })
}

/// Whether a Go `closed` event's metadata says a void closed it.
pub(crate) fn closed_by_void(metadata: &Value) -> bool {
    metadata.get("closed_by_void").and_then(Value::as_bool) == Some(true)
        || ["cause", "reason"].iter().any(|k| {
            matches!(
                metadata.get(*k).and_then(Value::as_str),
                Some("case_voided" | "voided")
            )
        })
}

/// Imports the server's appeals, then their events.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let first_decided = first_decisions(cx).await?;
    let g = cx.g.ulid.clone();
    let mut problems = Vec::new();
    let mut after = String::new();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM appeals WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "appeals", "id").key().to_owned();
        for row in &rows {
            let c = Cols::new(row, "appeals", "id");
            if let Err(p) = appeal(cx, &c, &first_decided).await? {
                problems.push(format!("appeal {}: {p}", c.key()));
            }
        }
        if rows.len() < BATCH {
            break;
        }
    }
    if !problems.is_empty() {
        return Err(ImportError::Server(problems));
    }
    events(cx).await
}

async fn first_decisions(cx: &mut Ctx<'_>) -> Result<HashMap<Uuid, OffsetDateTime>, ImportError> {
    let rows = cx
        .src
        .rows(
            "SELECT appeal_id, MIN(created_at) AS at FROM appeal_events
             WHERE guild_id = ? AND event_type IN ('accepted', 'rejected', 'closed') GROUP BY appeal_id",
            &[&cx.g.ulid.clone()],
        )
        .await?;
    let mut out = HashMap::new();
    for row in &rows {
        let c = Cols::new(row, "appeal_events", "appeal_id");
        if let Some(id) = c.uuid("appeal_id")? {
            out.insert(id, c.time("at")?);
        }
    }
    Ok(out)
}

async fn appeal(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    first: &HashMap<Uuid, OffsetDateTime>,
) -> Result<Result<(), String>, ImportError> {
    let key = cx.key(c.key());
    let Some(id) = c.uuid("id")? else {
        cx.f.skip("appeals", "invalid_id", &key);
        return Ok(Ok(()));
    };
    let Some(case_id) = c.uuid("case_id")? else {
        cx.f.skip("appeals", "null_case_appeal", &key);
        return Ok(Ok(()));
    };
    let Some(case) = cx.cases.get(&case_id).cloned() else {
        cx.f.skip("appeals", "missing_parent", &key);
        return Ok(Ok(()));
    };
    if c.snowflake("target_discord_user_id")? != Snowflake::Id(case.member) {
        cx.f.skip("appeals", "member_mismatch", &key);
        return Ok(Ok(()));
    }
    let state = c.text("status")?;
    if !matches!(
        state.as_str(),
        "pending" | "needs_information" | "accepted" | "rejected" | "closed"
    ) {
        return Ok(Err(format!("unknown status {state:?}")));
    }
    let Ok(version) = i32::try_from(c.unsigned("version")?) else {
        return Ok(Err("version is out of range".to_owned()));
    };
    let shape = &cx.src.shape;
    let has = |column: &str| shape.has("appeals", column);
    let answers = if has("answers_json") {
        c.json("answers_json")?
    } else {
        Value::Null
    };
    let (text, from_answers) = statement(
        if has("statement") {
            c.opt_text("statement")?
        } else {
            None
        },
        if has("content") {
            c.opt_text("content")?
        } else {
            None
        },
        Some(&answers),
    );
    if has("content") && !has("statement") {
        cx.f.derived("appeal_statement_from_content", &key);
    }
    if from_answers && !text.is_empty() {
        cx.f.derived("appeal_statement_from_answers", &key);
    }
    let (mut text, changed) = convert::clean(text);
    if changed {
        cx.f.normalized("appeals.statement", &key);
    }
    if text.trim().is_empty() {
        cx.f.derived("empty_appeal_statement", &key);
        NO_TEXT.clone_into(&mut text);
    }
    if convert::chars(&text) > 4_000 {
        cx.f.over_limit("appeals.statement", &key);
    }
    let reason = c.nonempty("decision_reason")?.map(|r| convert::clean(r).0);
    let created_at = c.time("created_at")?;
    let updated_at = c.time("updated_at")?;
    let q = sqlx::query(
        "INSERT INTO appeals (id, guild_id, case_id, member_id, state, version, statement, decided_by, decided_at,
                              decision_reason, first_decided_at, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5::appeal_state, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(case_id)
    .bind(case.member)
    .bind(&state)
    .bind(version.max(1))
    .bind(text)
    .bind(c.snowflake("reviewed_by_discord_user_id")?.id())
    .bind(c.opt_time("reviewed_at")?)
    .bind(reason)
    .bind(first.get(&id))
    .bind(created_at)
    .bind(updated_at);
    strict(&mut cx.tx, &mut cx.f, "appeals", &key, q).await?;
    cx.appeals.insert(
        id,
        AppealInfo {
            case: case_id,
            member: case.member,
            open: matches!(state.as_str(), "pending" | "needs_information"),
            review_message: c.snowflake("review_message_discord_id")?.id(),
            created_at,
            updated_at,
        },
    );
    Ok(Ok(()))
}

async fn events(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM appeal_events WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "appeal_events", "id").key().to_owned();
        for row in &rows {
            event(cx, &Cols::new(row, "appeal_events", "id")).await?;
        }
        if rows.len() < BATCH {
            break;
        }
    }
    Ok(())
}

async fn event(cx: &mut Ctx<'_>, c: &Cols<'_>) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let (Some(id), Some(appeal)) = (c.uuid("id")?, c.uuid("appeal_id")?) else {
        cx.f.skip("appeal_events", "invalid_id", &key);
        return Ok(());
    };
    if !cx.appeals.contains_key(&appeal) {
        cx.f.skip("appeal_events", "missing_parent", &key);
        return Ok(());
    }
    let raw = c.text("event_type")?;
    let Some(kind) = event_kind(&raw) else {
        cx.f.skip("appeal_events", "unknown_event_type", &key);
        return Ok(());
    };
    let by_void = kind == "closed" && closed_by_void(&c.json("metadata_json")?);
    let actor = match c.snowflake("actor_discord_user_id")? {
        _ if by_void => None,
        Snowflake::Invalid => {
            cx.f.skip("appeal_events", "invalid_snowflake", format!("{key}:actor"));
            None
        }
        s => s.id(),
    };
    let (mut text, changed) = convert::clean(c.text("body")?);
    if changed {
        cx.f.normalized("appeal_events.text", &key);
    }
    if text.trim().is_empty() {
        NO_TEXT.clone_into(&mut text);
    }
    let q = sqlx::query(
        "INSERT INTO appeal_events (id, guild_id, appeal_id, kind, actor_id, text, closed_by_void, created_at)
         VALUES ($1, $2, $3, $4::appeal_event_kind, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(appeal)
    .bind(kind)
    .bind(actor)
    .bind(text)
    .bind(by_void)
    .bind(c.time("created_at")?);
    if guarded(&mut cx.tx, &mut cx.f, "appeal_events", &key, q).await? {
        cx.appeal_events.insert(id, appeal);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_statements() {
        assert_eq!(
            statement(Some("now".into()), None, None),
            ("now".into(), false)
        );
        assert_eq!(
            statement(None, Some("old".into()), None),
            ("old".into(), false)
        );
        let answers =
            json!([{"value": " first "}, {"value": ""}, {"value": 3}, {"value": "second"}]);
        assert_eq!(
            statement(Some(String::new()), None, Some(&answers)),
            ("first\n\nsecond".into(), true)
        );
        assert_eq!(
            statement(None, None, Some(&json!("garbage"))),
            (String::new(), true)
        );
    }

    #[test]
    fn event_kinds() {
        assert_eq!(
            event_kind("information_submitted"),
            Some("information_provided")
        );
        assert_eq!(event_kind("nope"), None);
        assert!(closed_by_void(&json!({"cause": "case_voided"})));
        assert!(!closed_by_void(&json!({})));
    }
}
