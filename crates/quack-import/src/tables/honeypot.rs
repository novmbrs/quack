//! `honeypot_triggers` → `honeypot_hits`, with `honeypot_message_cleanups`
//! folded in (§19.4, §19.9). A pending trigger never becomes work Quack acts
//! on after cutover unless Go already made its case.

use std::collections::HashMap;

use quack_core::ids::{ChannelId, MessageId};
use quack_db::jobs::NewJob;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ImportError;
use crate::convert::Snowflake;
use crate::guild::Ctx;
use crate::source::{Cols, Pages};
use crate::target::{self, guarded};

/// A hit's state and failure from the Go outcome (§19.4). `pending_case`
/// is the case Go made under its operation key, if any.
pub(crate) fn hit(
    outcome: &str,
    failure_code: Option<&str>,
    case: Option<Uuid>,
    pending_case: Option<Uuid>,
) -> (Option<Uuid>, Option<String>) {
    match outcome {
        "created" => match case {
            Some(case) => (Some(case), None),
            None => (None, Some("imported_missing_case".to_owned())),
        },
        "exempt" => (None, Some("exempt".to_owned())),
        "pending" => match pending_case {
            Some(case) => (Some(case), None),
            None => (None, Some("imported_pending".to_owned())),
        },
        _ => (
            None,
            Some(
                failure_code
                    .filter(|c| !c.is_empty())
                    .unwrap_or("failed")
                    .chars()
                    .take(64)
                    .collect(),
            ),
        ),
    }
}

/// Imports the server's honeypot hits.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let cleanups = cleanups(cx).await?;
    let g = cx.g.ulid.clone();
    let mut pages = Pages::new(
        "SELECT * FROM honeypot_triggers WHERE guild_id = ? AND id > ? ORDER BY id",
        "honeypot_triggers",
        "id",
    );
    while let Some(rows) = pages.next(cx.src, &[&g]).await? {
        for row in &rows {
            trigger(cx, &Cols::new(row, "honeypot_triggers", "id"), &cleanups).await?;
        }
    }
    Ok(())
}

/// `message ID → completed_at` (NULL while the cleanup is unfinished).
async fn cleanups(
    cx: &mut Ctx<'_>,
) -> Result<HashMap<String, Option<OffsetDateTime>>, ImportError> {
    let rows = cx
        .src
        .rows("SELECT message_discord_id, completed_at FROM honeypot_message_cleanups WHERE guild_id = ?", &[&cx.g.ulid.clone()])
        .await?;
    let mut out = HashMap::new();
    for row in &rows {
        let c = Cols::new(row, "honeypot_message_cleanups", "message_discord_id");
        out.insert(c.key().trim().to_owned(), c.opt_time("completed_at")?);
    }
    Ok(out)
}

async fn trigger(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    cleanups: &HashMap<String, Option<OffsetDateTime>>,
) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let (Snowflake::Id(channel), Snowflake::Id(message), Snowflake::Id(member)) = (
        c.snowflake("channel_discord_id")?,
        c.snowflake("message_discord_id")?,
        c.snowflake("target_discord_user_id")?,
    ) else {
        cx.f.skip("honeypot_triggers", "invalid_snowflake", &key);
        return Ok(());
    };
    let rule = c
        .uuid("template_id")?
        .and_then(|t| cx.rule_ref.get(&t).copied());
    let outcome = c.text("outcome")?;
    let operation_key = format!("honeypot:{}:{message}", cx.g.ulid);
    let pending_case = cx.honeypot_keys.get(&operation_key).copied();
    let linked = c
        .uuid("case_id")?
        .filter(|id| cx.cases.get(id).is_some_and(|k| k.member == member));
    let (case, failure) = hit(
        &outcome,
        c.nonempty("failure_code")?.as_deref(),
        linked,
        pending_case,
    );
    match outcome.as_str() {
        "pending" if case.is_some() => cx.f.in_flight("honeypot_pending_case_found", &key),
        "pending" => cx.f.in_flight("honeypot_pending_failed", &key),
        "created" if case.is_none() => {
            cx.f.skip("honeypot_triggers.case_id", "missing_parent", &key);
        }
        _ => {}
    }
    let cleaned = cleanups.get(&message.to_string()).copied();
    let q = sqlx::query(
        "INSERT INTO honeypot_hits (guild_id, message_id, channel_id, member_id, settings_version, rule_id, state, case_id,
                                    failure, cleaned_up_at, created_at, updated_at)
         VALUES ($1, $2, $3, $4, 1, $5, $6::honeypot_hit_state, $7, $8, $9, $10, $11)",
    )
    .bind(cx.g.id)
    .bind(message)
    .bind(channel)
    .bind(member)
    .bind(rule)
    .bind(if case.is_some() { "case_created" } else { "failed" })
    .bind(case)
    .bind(failure)
    .bind(cleaned.flatten())
    .bind(c.time("created_at")?)
    .bind(c.time("updated_at")?);
    if !guarded(&mut cx.tx, &mut cx.f, "honeypot_hits", &key, q).await? {
        return Ok(());
    }
    // Deleting the bait is idempotent: finish the cleanups Go started and
    // left unfinished for hits that made a case.
    if case.is_some() && cleaned == Some(None) {
        let (Ok(channel), Ok(message)) =
            (ChannelId::try_from(channel), MessageId::try_from(message))
        else {
            return Ok(());
        };
        let job = NewJob::honeypot_cleanup(target::guild(cx.g.id)?, channel, message);
        target::job(&mut cx.tx, &mut cx.f, job).await?;
        cx.f.in_flight("honeypot_cleanup_enqueued", &key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits() {
        let case = Uuid::from_u128(1);
        assert_eq!(hit("created", None, Some(case), None), (Some(case), None));
        assert_eq!(
            hit("failed", Some("template_unavailable"), None, None)
                .1
                .as_deref(),
            Some("template_unavailable")
        );
        assert_eq!(
            hit("failed", Some(""), None, None).1.as_deref(),
            Some("failed")
        );
        assert_eq!(hit("exempt", None, None, None).1.as_deref(), Some("exempt"));
        assert_eq!(hit("pending", None, None, Some(case)), (Some(case), None));
        assert_eq!(
            hit("pending", None, None, None).1.as_deref(),
            Some("imported_pending")
        );
    }
}
