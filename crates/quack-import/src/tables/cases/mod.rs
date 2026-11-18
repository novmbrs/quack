//! `cases` → `cases` (§19.6). v4 cases become historical, non-counting
//! `imported` cases; every other case keeps its rule version and decision
//! from its snapshot. Replacement links are set after every case exists.

mod transform;

use std::collections::{HashMap, HashSet};

use quack_core::rule::LevelAction;
use uuid::Uuid;

use self::transform::{Claim, Dropped};
use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::{CaseInfo, Ctx};
use crate::source::{BATCH, Cols};
use crate::tables::rules::snapshot::{self, Snapshot};
use crate::target::strict;

/// The text a void with an empty reason gets.
pub(crate) const NO_VOID_REASON: &str = "(no reason recorded)";

/// The largest case number PostgreSQL and JSON clients accept (§19.3).
const MAX_CASE_NUMBER: u64 = (1 << 53) - 1;

/// One case row to insert.
struct Row {
    id: Uuid,
    number: i64,
    member: i64,
    source: &'static str,
    moderator: Option<i64>,
    rule: Option<(Uuid, i32)>,
    case_count: Option<i32>,
    level_index: Option<i16>,
    level_from: Option<i32>,
    action: Option<LevelAction>,
    sends_dm: bool,
    context: serde_json::Value,
    operation_key: Option<String>,
    void: Option<(time::OffsetDateTime, Option<i64>, String, &'static str)>,
    imported: Option<serde_json::Value>,
    created_at: time::OffsetDateTime,
    updated_at: time::OffsetDateTime,
}

/// Imports every case of the server.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let accepted = accepted_appeal_cases(cx).await?;
    let mut problems = Vec::new();
    let mut claims = Vec::new();
    let mut after = String::new();
    let g = cx.g.ulid.clone();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM cases WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "cases", "id").key().to_owned();
        for row in &rows {
            let c = Cols::new(row, "cases", "id");
            match case_row(cx, &c, &accepted)? {
                Err(problem) => problems.push(format!("case {}: {problem}", c.key())),
                Ok(None) => {}
                Ok(Some(r)) => {
                    if let Some(target) = c.uuid("replaces_case_id")? {
                        claims.push(Claim { case: r.id, target });
                    }
                    if !problems.is_empty() {
                        continue;
                    }
                    insert(cx, &c, r).await?;
                }
            }
        }
        if rows.len() < BATCH {
            break;
        }
    }
    if !problems.is_empty() {
        return Err(ImportError::Server(problems));
    }
    link_replacements(cx, &claims).await
}

async fn accepted_appeal_cases(cx: &mut Ctx<'_>) -> Result<HashSet<Uuid>, ImportError> {
    let rows = cx
        .src
        .rows(
            "SELECT case_id FROM appeals WHERE guild_id = ? AND status = 'accepted' AND case_id IS NOT NULL",
            &[&cx.g.ulid.clone()],
        )
        .await?;
    let mut out = HashSet::new();
    for row in &rows {
        if let Some(id) = Cols::new(row, "appeals", "case_id").uuid("case_id")? {
            out.insert(id);
        }
    }
    Ok(out)
}

/// Maps one case; `Ok(Err(_))` aborts the server, `Ok(Ok(None))` skips it.
#[expect(
    clippy::too_many_lines,
    reason = "one case's mapping reads best top to bottom, column by column"
)]
fn case_row(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    accepted: &HashSet<Uuid>,
) -> Result<Result<Option<Row>, String>, ImportError> {
    let key = cx.key(c.key());
    let Some(id) = c.uuid("id")? else {
        cx.f.skip("cases", "invalid_id", &key);
        return Ok(Ok(None));
    };
    let number = c.unsigned("case_number")?;
    if !(1..=MAX_CASE_NUMBER).contains(&number) {
        return Ok(Err(format!("case number {number} is out of range")));
    }
    let Snowflake::Id(member) = c.snowflake("target_discord_user_id")? else {
        cx.f.skip("cases", "invalid_snowflake", &key);
        return Ok(Ok(None));
    };
    let moderator = match c.snowflake("moderator_discord_user_id")? {
        // The case is written without its moderator: a repaired value,
        // not a skipped case.
        Snowflake::Invalid => {
            cx.f.derived("case_moderator_invalid", &key);
            None
        }
        s => s.id(),
    };
    let source = match c.text("source")?.as_str() {
        "discord" => "discord",
        "dashboard" => "dashboard",
        "honeypot" => "honeypot",
        "v4_import" => "imported",
        other => return Ok(Err(format!("unknown source {other:?}"))),
    };
    // The schema lets an imported Discord or dashboard case lack its
    // moderator (its operation key is absent or `legacy:`).
    if !matches!(source, "honeypot" | "imported") && moderator.is_none() {
        cx.f.derived("case_moderator_unknown", &key);
    }
    let created_at = c.time("created_at")?;
    let (context, notes) = transform::context(&c.json("context_values_json")?);
    if notes.not_an_array {
        cx.f.derived("context_not_an_array", &key);
    }
    if notes.duplicate_key {
        cx.f.derived("context_duplicate_key", &key);
    }
    if notes.number_kept {
        cx.f.derived("context_number_kept_as_text", &key);
    }
    let void = void(cx, c, &key, id, accepted)?;
    let operation_key = c.nonempty("idempotency_key")?;
    if let Some(k) = &operation_key
        && k.starts_with("honeypot:")
    {
        cx.honeypot_keys.insert(k.clone(), id);
    }
    let mut row = Row {
        id,
        number: i64::try_from(number).unwrap_or(i64::MAX),
        member,
        source,
        moderator,
        rule: None,
        case_count: None,
        level_index: None,
        level_from: None,
        action: None,
        sends_dm: false,
        context,
        operation_key: operation_key.map(|k| format!("legacy:{k}")),
        void,
        imported: None,
        created_at,
        updated_at: c.time("updated_at")?,
    };
    if source == "imported" {
        row.imported = Some(transform::v4_imported(
            &c.json("metadata_json")?,
            &c.text("reason")?,
        ));
        return Ok(Ok(Some(row)));
    }
    let snap = Snapshot::parse(&c.json("template_snapshot_json")?);
    if snap.template.is_none() {
        return Ok(Err(format!("case #{number}: the snapshot has no template")));
    }
    let decision = match snapshot::decision(&snap) {
        Ok(d) => d,
        Err(e) => return Ok(Err(format!("case #{number}: {e}"))),
    };
    let template = c.uuid("template_id")?.or_else(|| {
        snap.template
            .as_ref()
            .and_then(|t| t.id.as_deref())
            .and_then(convert::uuid)
    });
    let rule = template.and_then(|t| cx.rule_ref.get(&t).copied());
    let version = u32::try_from(c.unsigned("template_version")?).ok();
    let (Some(rule), Some(version)) = (rule, version) else {
        return Ok(Err(format!("case #{number}: its rule was not imported")));
    };
    let Some(levels) = cx.versions.get(&(rule, version)) else {
        return Ok(Err(format!(
            "case #{number}: rule version {version} was not imported"
        )));
    };
    let from = decision.level.from_case_count;
    let index = decision.position.or_else(|| {
        levels
            .iter()
            .position(|l| *l == from)
            .and_then(|i| i16::try_from(i + 1).ok())
    });
    row.rule = Some((rule, i32::try_from(version).unwrap_or(i32::MAX)));
    row.case_count = Some(decision.case_count);
    row.level_index = Some(index.unwrap_or(1));
    row.level_from = Some(i32::try_from(from).unwrap_or(i32::MAX));
    row.action = decision.level.action;
    row.sends_dm = decision.level.send_dm;
    Ok(Ok(Some(row)))
}

type Void = Option<(time::OffsetDateTime, Option<i64>, String, &'static str)>;

/// The void columns; `validity` decides, as it did for Go's counting, and
/// disagreements with `voided_at` are reported.
fn void(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    key: &str,
    id: Uuid,
    accepted: &HashSet<Uuid>,
) -> Result<Void, ImportError> {
    let validity = c.text("validity")?;
    let voided_at = c.opt_time("voided_at")?;
    let voided = match validity.as_str() {
        "voided" => true,
        "valid" => false,
        _ => {
            cx.f.derived("unknown_validity", key);
            voided_at.is_some()
        }
    };
    if voided != voided_at.is_some() {
        cx.f.derived("validity_disagrees_with_voided_at", key);
    }
    if !voided {
        return Ok(None);
    }
    let at = voided_at.map_or_else(|| c.time("updated_at"), Ok)?;
    let reason = if let Some(r) = c.nonempty("voided_reason")? {
        convert::clean(r).0
    } else {
        cx.f.derived("empty_void_reason", key);
        NO_VOID_REASON.to_owned()
    };
    let by = c.snowflake("voided_by_discord_user_id")?.id();
    let cause = if accepted.contains(&id) {
        "appeal_accepted"
    } else {
        "staff"
    };
    Ok(Some((at, by, reason, cause)))
}

async fn insert(cx: &mut Ctx<'_>, c: &Cols<'_>, r: Row) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let (kind, timeout, ban) = match r.action {
        None => (None, None, None),
        Some(LevelAction::Timeout { duration_seconds }) => {
            (Some("timeout"), i32::try_from(duration_seconds).ok(), None)
        }
        Some(LevelAction::Kick) => (Some("kick"), None, None),
        Some(LevelAction::Ban {
            delete_message_seconds,
        }) => (
            Some("ban"),
            None,
            i32::try_from(delete_message_seconds).ok(),
        ),
    };
    let (voided_at, voided_by, void_reason, void_cause) = match &r.void {
        Some((at, by, reason, cause)) => (Some(*at), *by, Some(reason.clone()), Some(*cause)),
        None => (None, None, None, None),
    };
    let q = sqlx::query(
        "INSERT INTO cases (id, guild_id, number, member_id, member_joined_at, source, moderator_id, rule_id, rule_version,
                            case_count, level_index, level_from_case_count, action_kind, timeout_seconds,
                            ban_delete_message_seconds, sends_dm, context, operation_key, voided_at, voided_by,
                            void_reason, void_cause, imported, created_at, updated_at)
         VALUES ($1, $2, $3, $4, NULL, $5::case_source, $6, $7, $8, $9, $10, $11, $12::action_kind, $13, $14, $15, $16,
                 $17, $18, $19, $20, $21::void_cause, $22, $23, $24)",
    )
    .bind(r.id)
    .bind(cx.g.id)
    .bind(r.number)
    .bind(r.member)
    .bind(r.source)
    .bind(r.moderator)
    .bind(r.rule.map(|(id, _)| id))
    .bind(r.rule.map(|(_, v)| v))
    .bind(r.case_count)
    .bind(r.level_index)
    .bind(r.level_from)
    .bind(kind)
    .bind(timeout)
    .bind(ban)
    .bind(r.sends_dm)
    .bind(&r.context)
    .bind(&r.operation_key)
    .bind(voided_at)
    .bind(voided_by)
    .bind(void_reason)
    .bind(void_cause)
    .bind(&r.imported)
    .bind(r.created_at)
    .bind(r.updated_at);
    strict(&mut cx.tx, &mut cx.f, "cases", &key, q).await?;
    cx.max_case_number = cx.max_case_number.max(u64::try_from(r.number).unwrap_or(0));
    cx.cases.insert(
        r.id,
        CaseInfo {
            member: r.member,
            voided: r.void.is_some(),
            created_at: r.created_at,
            timeout_seconds: timeout,
            ban_delete_seconds: ban,
        },
    );
    Ok(())
}

/// Sets every valid `replaces_case_id` and reports the rest.
async fn link_replacements(cx: &mut Ctx<'_>, claims: &[Claim]) -> Result<(), ImportError> {
    let voided: HashMap<Uuid, bool> = cx
        .cases
        .iter()
        .map(|(id, info)| (*id, info.voided))
        .collect();
    let claims: Vec<Claim> = claims
        .iter()
        .filter(|c| cx.cases.contains_key(&c.case))
        .copied()
        .collect();
    let (kept, dropped) = transform::replacements(&claims, &voided);
    for (claim, why) in dropped {
        let reason = match why {
            Dropped::NotInServer => "not_in_server",
            Dropped::Itself => "itself",
            Dropped::NotVoided => "not_voided",
            Dropped::ReplacedTwice => "replaced_twice",
            Dropped::Cycle => "cycle",
        };
        cx.f.skip(
            "cases.replaces_case_id",
            "invalid_replacement",
            format!("{}:{}:{reason}", cx.g.id, claim.case),
        );
        if why == Dropped::Cycle {
            cx.f.check("replacement_cycles", vec![claim.case.to_string()]);
        }
    }
    for claim in kept {
        sqlx::query("UPDATE cases SET replaces_case_id = $2 WHERE id = $1")
            .bind(claim.case)
            .bind(claim.target)
            .execute(&mut *cx.tx)
            .await
            .map_err(ImportError::target)?;
    }
    Ok(())
}
