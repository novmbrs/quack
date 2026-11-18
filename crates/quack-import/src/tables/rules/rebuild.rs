//! Rule versions rebuilt from case snapshots (§19.5 step 3), and rules
//! whose template is gone, rebuilt from their cases.

use std::collections::BTreeMap;

use quack_core::rule::RuleDefinition;
use sqlx::Row as _;
use sqlx::mysql::MySqlRow;
use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use super::snapshot::{self, Rebuild, Snapshot};
use super::{RuleRow, version_of};
use crate::ImportError;
use crate::convert;
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};

/// Scans the cases that used a version other than their template's
/// current one (or a template that is gone), in creation order, and
/// rebuilds those versions. Missing templates become archived rules.
pub(super) async fn scan_snapshots(
    cx: &mut Ctx<'_>,
    rules: &mut Vec<RuleRow>,
    problems: &mut Vec<String>,
) -> Result<BTreeMap<(Uuid, u32), Rebuild>, ImportError> {
    const SQL: &str = "SELECT c.id, c.case_number, c.created_at, c.template_id, c.template_version,
                              c.template_snapshot_json, c.reason
                       FROM cases c LEFT JOIN case_templates t ON t.id = c.template_id AND t.guild_id = c.guild_id
                       WHERE c.guild_id = ? AND c.source <> 'v4_import'
                         AND (t.id IS NULL OR c.template_version <> t.version)
                         AND (? IS NULL OR c.created_at > ? OR (c.created_at = ? AND c.id > ?))
                       ORDER BY c.created_at, c.id";
    let mut rebuilds: BTreeMap<(Uuid, u32), Rebuild> = BTreeMap::new();
    let mut after: (Option<PrimitiveDateTime>, String) = (None, String::new());
    loop {
        let rows: Vec<MySqlRow> = sqlx::query(sqlx::AssertSqlSafe(format!("{SQL} LIMIT {BATCH}")))
            .bind(&cx.g.ulid)
            .bind(after.0)
            .bind(after.0)
            .bind(after.0)
            .bind(&after.1)
            .fetch_all(cx.src.conn())
            .await
            .map_err(ImportError::source)?;
        let Some(last) = rows.last() else { break };
        after = (
            Some(last.try_get("created_at").map_err(ImportError::source)?),
            last.try_get("id").map_err(ImportError::source)?,
        );
        for row in &rows {
            let c = Cols::new(row, "cases", "id");
            if let Err(p) = snapshot_case(cx, &c, rules, &mut rebuilds).await? {
                problems.push(format!(
                    "case #{} ({}): {p}",
                    c.unsigned("case_number")?,
                    c.key()
                ));
            }
        }
        if rows.len() < BATCH {
            break;
        }
    }
    Ok(rebuilds)
}

async fn snapshot_case(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    rules: &mut Vec<RuleRow>,
    rebuilds: &mut BTreeMap<(Uuid, u32), Rebuild>,
) -> Result<Result<(), String>, ImportError> {
    let snap = Snapshot::parse(&c.json("template_snapshot_json")?);
    let template_key = c
        .nonempty("template_id")?
        .or_else(|| snap.template.as_ref().and_then(|t| t.id.clone()));
    let Some(template_key) = template_key else {
        return Ok(Err(
            "neither template_id nor the snapshot names its rule".to_owned()
        ));
    };
    let Some(version) = version_of(c.unsigned("template_version")?) else {
        return Ok(Err("template_version is out of range".to_owned()));
    };
    let decision = match snapshot::decision(&snap) {
        Ok(d) => d,
        Err(e) => return Ok(Err(e)),
    };
    let part = match snapshot::definition_part(&snap, c.text("reason")?.trim()) {
        Ok(p) => p,
        Err(e) => return Ok(Err(e)),
    };
    let Some(mapped) = convert::uuid(&template_key) else {
        return Ok(Err("its rule ID is not a ULID".to_owned()));
    };
    let rule = match cx.rule_ref.get(&mapped) {
        Some(rule) => *rule,
        None => {
            missing_rule(
                cx,
                rules,
                &template_key,
                mapped,
                &part,
                c.time("created_at")?,
            )
            .await?
        }
    };
    let at = c.time("created_at")?;
    match rebuilds.get_mut(&(rule, version)) {
        Some(r) => r.add(&part, decision.level, c.key()),
        None => {
            rebuilds.insert((rule, version), Rebuild::new(part, decision.level, at));
        }
    }
    if let Some(row) = rules.iter_mut().find(|r| r.id == rule) {
        row.updated_at = row.updated_at.max(at);
    }
    Ok(Ok(()))
}

/// A rule whose template is gone (or belongs to another server): rebuilt
/// from its cases as an archived rule so no case is lost.
async fn missing_rule(
    cx: &mut Ctx<'_>,
    rules: &mut Vec<RuleRow>,
    template_key: &str,
    mapped: Uuid,
    part: &RuleDefinition,
    at: OffsetDateTime,
) -> Result<Uuid, ImportError> {
    let elsewhere = cx
        .src
        .rows(
            "SELECT guild_id FROM case_templates WHERE id = ?",
            &[template_key],
        )
        .await?;
    let id = if elsewhere.is_empty() {
        cx.f.derived("rule_rebuilt_from_cases", template_key);
        mapped
    } else {
        // Its ID belongs to another server's rule: give this server's copy
        // a different (deterministic) ID.
        cx.f.derived("cross_guild_rule_rebuilt", template_key);
        derived_id(mapped, cx.g.id)
    };
    cx.rule_ref.insert(mapped, id);
    let name = if part.name.trim().is_empty() {
        format!("Rule {template_key}")
    } else {
        part.name.clone()
    };
    rules.push(RuleRow {
        id,
        key: template_key.to_owned(),
        name,
        current_version: 0,
        archived_at: Some(cx.evaluated_at),
        created_at: at,
        created_by: None,
        updated_at: at,
        updated_by: None,
        rebuilt: true,
        versions: BTreeMap::new(),
    });
    Ok(id)
}

/// A deterministic UUID for a server's copy of another server's rule.
pub(crate) fn derived_id(id: Uuid, guild: i64) -> Uuid {
    let mask = (1u128 << 62) - 1;
    Uuid::from_u128(id.as_u128() ^ (u128::from(guild.unsigned_abs()) & mask))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_stay_version_seven() {
        let id = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
        let d = derived_id(id, 123_456_789);
        assert_ne!(d, id);
        assert_eq!(d.get_version_num(), 7);
        assert_eq!(derived_id(id, 123_456_789), d);
    }
}
