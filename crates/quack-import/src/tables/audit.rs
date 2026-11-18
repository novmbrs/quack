//! `audit_log_entries` → `mod_log` (§19.8): every row, kind `legacy`, never
//! re-posted, with typed links filled from the resource and metadata and
//! checked against each other.

use quack_core::modlog::{LegacyDetail, redact};
use serde_json::Value;
use uuid::Uuid;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};
use crate::target::guarded;

/// Go results → `mod_log_result`.
pub(crate) fn result(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "success" => "ok",
        "failure" => "failed",
        "denied" => "refused",
        _ => return None,
    })
}

/// The redacted legacy detail (§9.5).
pub(crate) fn detail(
    action: &str,
    resource_type: &str,
    resource_id: &str,
    failure_reason: Option<&str>,
) -> Value {
    let id = resource_id.trim();
    let d = LegacyDetail {
        action: action.to_owned(),
        resource_type: resource_type.to_owned(),
        resource_id: (!id.is_empty() && id != "unknown").then(|| id.to_owned()),
        failure_reason: failure_reason
            .filter(|r| !r.trim().is_empty())
            .map(str::to_owned),
    };
    redact(serde_json::to_value(d).unwrap_or_else(|_| Value::Object(serde_json::Map::new())))
}

/// The typed links of one entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Links {
    case: Option<Uuid>,
    member: Option<i64>,
    appeal: Option<Uuid>,
    action: Option<Uuid>,
    rule: Option<Uuid>,
    ticket: Option<Uuid>,
}

/// Imports the server's moderation log.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM audit_log_entries WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "audit_log_entries", "id").key().to_owned();
        for row in &rows {
            entry(cx, &Cols::new(row, "audit_log_entries", "id")).await?;
        }
        if rows.len() < BATCH {
            break;
        }
    }
    Ok(())
}

async fn entry(cx: &mut Ctx<'_>, c: &Cols<'_>) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let Some(id) = c.uuid("id")? else {
        cx.f.skip("audit_log_entries", "invalid_id", &key);
        return Ok(());
    };
    let raw_result = c.text("result")?;
    let Some(result) = result(&raw_result) else {
        cx.f.skip("audit_log_entries", "unknown_result", &key);
        return Ok(());
    };
    let action = c.text("action")?;
    let resource_type = c.text("resource_type")?;
    let resource_id = c.text("resource_id")?;
    let links = links(
        cx,
        &key,
        &resource_type,
        &resource_id,
        &c.json("metadata_json")?,
    );
    let actor = match c.snowflake("actor_discord_user_id")? {
        Snowflake::Invalid => {
            cx.f.skip(
                "audit_log_entries",
                "invalid_snowflake",
                format!("{key}:actor"),
            );
            None
        }
        s => s.id(),
    };
    let detail = detail(
        &action,
        &resource_type,
        &resource_id,
        c.opt_text("failure_reason")?.as_deref(),
    );
    let q = sqlx::query(
        "INSERT INTO mod_log (id, guild_id, kind, result, actor_id, source, case_id, member_id, appeal_id, action_id,
                              rule_id, ticket_id, detail, trace_id, created_at)
         VALUES ($1, $2, 'legacy', $3::mod_log_result, $4, 'imported', $5, $6, $7, $8, $9, $10, $11, NULL, $12)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(result)
    .bind(actor)
    .bind(links.case)
    .bind(links.member)
    .bind(links.appeal)
    .bind(links.action)
    .bind(links.rule)
    .bind(links.ticket)
    .bind(detail)
    .bind(c.time("created_at")?);
    if guarded(&mut cx.tx, &mut cx.f, "mod_log", &key, q).await? {
        *cx.f.legacy_mod_log.entry(action).or_default() += 1;
    }
    Ok(())
}

/// Fills the links from `resource_type`/`resource_id` and the metadata keys
/// §19.8 names, keeps only imported rows, and drops a link that disagrees
/// with a stronger one (appeal, then case, then action, then member).
fn links(
    cx: &mut Ctx<'_>,
    key: &str,
    resource_type: &str,
    resource_id: &str,
    metadata: &Value,
) -> Links {
    let meta = |k: &str| metadata.get(k).and_then(Value::as_str);
    let ulid = |s: Option<&str>| s.and_then(convert::uuid);
    let by_resource = |t: &str| (resource_type == t).then_some(resource_id);
    let mut out = Links::default();
    let miss =
        |cx: &mut Ctx<'_>, column: &str, why: &str| cx.f.null_link(&format!("{column}:{why}"), key);

    if let Some(appeal) = ulid(by_resource("appeal")).or_else(|| ulid(meta("appeal_id"))) {
        if cx.appeals.contains_key(&appeal) {
            out.appeal = Some(appeal);
        } else {
            miss(cx, "appeal_id", "not_imported");
        }
    }
    if let Some(case) = ulid(by_resource("case")).or_else(|| ulid(meta("case_id"))) {
        match cx.appeals.get(&out.appeal.unwrap_or_default()) {
            Some(a) if a.case != case => miss(cx, "case_id", "disagrees"),
            _ if cx.cases.contains_key(&case) => out.case = Some(case),
            _ => miss(cx, "case_id", "not_imported"),
        }
    }
    if let Some(action) = ulid(by_resource("case_action_execution")) {
        match cx.actions.get(&action) {
            None => miss(cx, "action_id", "not_imported"),
            Some(a) if out.case.is_some_and(|c| c != a.case) => miss(cx, "action_id", "disagrees"),
            Some(_) => out.action = Some(action),
        }
    }
    let member = by_resource("member")
        .or_else(|| meta("target_discord_user_id"))
        .or_else(|| meta("member_discord_user_id"));
    if let Some(member) = convert::snowflake(member).id() {
        let expected = out
            .case
            .and_then(|c| cx.cases.get(&c).map(|k| k.member))
            .or_else(|| {
                out.appeal
                    .and_then(|a| cx.appeals.get(&a).map(|k| k.member))
            });
        match expected {
            Some(m) if m != member => miss(cx, "member_id", "disagrees"),
            _ => out.member = Some(member),
        }
    }
    if let Some(template) = ulid(by_resource("case_template")) {
        match cx.rule_ref.get(&template) {
            Some(rule) => out.rule = Some(*rule),
            None => miss(cx, "rule_id", "not_imported"),
        }
    }
    if let Some(ticket) = ulid(by_resource("ticket")) {
        if cx.tickets.contains_key(&ticket) {
            out.ticket = Some(ticket);
        } else {
            miss(cx, "ticket_id", "not_imported");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_and_details() {
        assert_eq!(result("success"), Some("ok"));
        assert_eq!(result("denied"), Some("refused"));
        assert_eq!(result("maybe"), None);
        let d = detail(
            "case.create",
            "case",
            "unknown",
            Some("Authorization: Bearer abc"),
        );
        assert_eq!(d["resource_id"], Value::Null);
        assert_eq!(d["failure_reason"], "[REDACTED]");
        assert_eq!(d["action"], "case.create");
        let long = "x".repeat(500);
        assert_eq!(
            detail("a", "b", "c", Some(&long))["failure_reason"]
                .as_str()
                .map(str::len),
            Some(240)
        );
    }
}
