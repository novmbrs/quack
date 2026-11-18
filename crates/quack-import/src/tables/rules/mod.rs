//! `case_templates` and their children → `rules` + immutable
//! `rule_versions` (§19.5): the current version from the child rows, older
//! versions rebuilt from the snapshots of the cases that used them.

pub(crate) mod current;
mod names;
pub(crate) mod snapshot;

use std::collections::{BTreeMap, HashMap};

use quack_core::rule::{RuleDefinition, Strictness};
use sqlx::Row as _;
use sqlx::mysql::MySqlRow;
use time::{OffsetDateTime, PrimitiveDateTime};
use uuid::Uuid;

use self::current::{FieldRow, LevelRow, Template};
use self::snapshot::{Rebuild, Snapshot};
use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::report::ReconstructedVersion;
use crate::source::{BATCH, Cols};
use crate::target::strict;

/// One rule to write.
#[derive(Debug)]
pub(crate) struct RuleRow {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub current_version: u32,
    pub archived_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
    pub created_by: Option<i64>,
    pub updated_at: OffsetDateTime,
    pub updated_by: Option<i64>,
    /// Rebuilt from cases because its template is gone.
    pub rebuilt: bool,
    /// `version → (definition, reconstructed, created_at, created_by)`.
    pub versions: BTreeMap<u32, (RuleDefinition, bool, OffsetDateTime, Option<i64>)>,
}

/// Imports the server's rules and every version its cases used.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let mut problems = Vec::new();
    let mut rules = current_rules(cx, &mut problems).await?;
    let rebuilds = scan_snapshots(cx, &mut rules, &mut problems).await?;
    if !problems.is_empty() {
        return Err(ImportError::Server(problems));
    }
    for ((rule, version), r) in rebuilds {
        let Some(row) = rules.iter_mut().find(|row| row.id == rule) else {
            continue;
        };
        let mut def = r.definition();
        if def.name.trim().is_empty() {
            def.name.clone_from(&row.name);
        }
        if let Err(found) = def.validate(Strictness::Imported {
            reconstructed: true,
        }) {
            for p in found {
                problems.push(format!(
                    "rule {} version {version}: {} ({})",
                    row.key,
                    p.path(),
                    p.code().as_str()
                ));
            }
            continue;
        }
        for case in &r.conflicts {
            cx.f.conflicting_snapshots.add(format!("{}:{case}", row.id));
        }
        cx.f.reconstructed_versions.push(ReconstructedVersion {
            rule_id: row.id.to_string(),
            version,
            known_levels: def.levels.len(),
        });
        if row.current_version < version {
            if !row.rebuilt {
                cx.f.derived(
                    "rule_version_after_current",
                    format!("{}:{version}", row.id),
                );
            }
            row.current_version = version;
        }
        row.versions
            .insert(version, (def, true, r.first_seen, None));
    }
    for row in rules.iter_mut().filter(|r| r.rebuilt) {
        if let Some((def, ..)) = row.versions.get(&row.current_version) {
            row.name.clone_from(&def.name);
        }
    }
    if !problems.is_empty() {
        return Err(ImportError::Server(problems));
    }
    for renamed in names::dedupe(&mut rules) {
        cx.f.renamed_rules.push(renamed);
    }
    for row in &rules {
        insert(cx, row).await?;
    }
    Ok(())
}

async fn insert(cx: &mut Ctx<'_>, row: &RuleRow) -> Result<(), ImportError> {
    let key = cx.key(&row.key);
    let q = sqlx::query(
        "INSERT INTO rules (id, guild_id, name, current_version, archived_at, created_at, created_by, updated_at, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(row.id)
    .bind(cx.g.id)
    .bind(&row.name)
    .bind(i32::try_from(row.current_version).unwrap_or(i32::MAX))
    .bind(row.archived_at)
    .bind(row.created_at)
    .bind(row.created_by)
    .bind(row.updated_at)
    .bind(row.updated_by);
    strict(&mut cx.tx, &mut cx.f, "rules", &key, q).await?;
    for (version, (def, reconstructed, at, by)) in &row.versions {
        let mut def = def.clone();
        if *version == row.current_version {
            // rules.name is the current version's name, renamed or not.
            def.name.clone_from(&row.name);
        }
        for code in current::over_limit(&def) {
            cx.f.over_limit(
                &format!("rule_versions.definition.{}", code.as_str()),
                format!("{}:{version}", row.id),
            );
        }
        let json = serde_json::to_value(&def)
            .map_err(|e| ImportError::server(format!("rule {}: {e}", row.key)))?;
        let q = sqlx::query(
            "INSERT INTO rule_versions (rule_id, version, guild_id, definition, reconstructed, created_at, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(row.id)
        .bind(i32::try_from(*version).unwrap_or(i32::MAX))
        .bind(cx.g.id)
        .bind(json)
        .bind(*reconstructed)
        .bind(*at)
        .bind(*by);
        strict(
            &mut cx.tx,
            &mut cx.f,
            "rule_versions",
            &format!("{key}:{version}"),
            q,
        )
        .await?;
        cx.versions.insert(
            (row.id, *version),
            def.levels.iter().map(|l| l.from_case_count).collect(),
        );
    }
    cx.rules.insert(row.id);
    Ok(())
}

/// A version number from a `bigint unsigned` column (§19.3).
fn version_of(raw: u64) -> Option<u32> {
    u32::try_from(raw)
        .ok()
        .filter(|v| (1..=i32::MAX.unsigned_abs()).contains(v))
}

fn actor(cx: &mut Ctx<'_>, c: &Cols<'_>, column: &str) -> Result<Option<i64>, ImportError> {
    Ok(match c.snowflake(column)? {
        Snowflake::Id(id) => Some(id),
        Snowflake::None => None,
        Snowflake::Invalid => {
            cx.f.skip(
                "case_templates",
                "invalid_snowflake",
                format!("{}:{column}", c.key()),
            );
            None
        }
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "reads a template and its three child tables in one place"
)]
async fn current_rules(
    cx: &mut Ctx<'_>,
    problems: &mut Vec<String>,
) -> Result<Vec<RuleRow>, ImportError> {
    let g = cx.g.ulid.clone();
    let templates = cx
        .src
        .rows(
            "SELECT * FROM case_templates WHERE guild_id = ? ORDER BY id",
            &[&g],
        )
        .await?;
    let field_rows = cx
        .src
        .rows(
            "SELECT f.* FROM case_template_context_fields f JOIN case_templates t ON t.id = f.template_id
             WHERE t.guild_id = ? ORDER BY f.template_id, f.position",
            &[&g],
        )
        .await?;
    let level_rows = cx
        .src
        .rows(
            "SELECT l.*, a.action_type, a.config_json AS action_config
             FROM case_template_levels l JOIN case_templates t ON t.id = l.template_id
             LEFT JOIN case_template_level_actions a ON a.level_id = l.id
             WHERE t.guild_id = ? ORDER BY l.template_id, l.position",
            &[&g],
        )
        .await?;
    let mut fields: HashMap<Uuid, Vec<FieldRow>> = HashMap::new();
    for row in &field_rows {
        let c = Cols::new(row, "case_template_context_fields", "id");
        let Some(t) = c.uuid("template_id")? else {
            continue;
        };
        fields.entry(t).or_default().push(FieldRow {
            key: c.text("key")?,
            label: c.text("label")?,
            field_type: c.text("field_type")?,
            position: c.int("position")?,
            required: c.flag("required")?,
        });
    }
    let mut levels: HashMap<Uuid, Vec<LevelRow>> = HashMap::new();
    for row in &level_rows {
        let c = Cols::new(row, "case_template_levels", "id");
        let Some(t) = c.uuid("template_id")? else {
            continue;
        };
        let is_default = c.flag("is_default")?;
        if is_default && c.uuid("default_template_id")? != Some(t) {
            cx.f.derived("default_template_id_mismatch", c.key());
        }
        let action = match c.opt_text("action_type")? {
            Some(kind) => Some((kind, c.json("action_config")?)),
            None => None,
        };
        levels.entry(t).or_default().push(LevelRow {
            position: c.int("position")?,
            is_default,
            trigger: c.int("trigger_case_count")?,
            notify: c.flag("notify_user")?,
            action,
        });
    }
    let mut out = Vec::new();
    for row in &templates {
        let c = Cols::new(row, "case_templates", "id");
        let key = c.key().to_owned();
        let Some(id) = c.uuid("id")? else {
            problems.push(format!("case_templates {key}: the ID is not a ULID"));
            continue;
        };
        let Some(version) = version_of(c.unsigned("version")?) else {
            problems.push(format!("case_templates {key}: version is out of range"));
            continue;
        };
        let t = Template {
            name: clean(cx, c.text("name")?, "rules.name", &key),
            description: clean(
                cx,
                c.text("description")?,
                "rule_versions.definition.description",
                &key,
            ),
            reason: clean(
                cx,
                c.text("reason_template")?,
                "rule_versions.definition.reason",
                &key,
            ),
            appealable: c.flag("appealable")?,
            decay_days: c.int("case_decay_days")?,
        };
        let (def, notes) = match current::build(
            &t,
            fields.get(&id).map_or(&[], Vec::as_slice),
            levels.get(&id).map_or(&[], Vec::as_slice),
        ) {
            Ok(built) => built,
            Err(found) => {
                problems.extend(
                    found
                        .into_iter()
                        .map(|p| format!("case_templates {key}: {p}")),
                );
                continue;
            }
        };
        if notes.dropped_default {
            cx.f.dropped_default_levels.add(id.to_string());
        }
        let updated_by = actor(cx, &c, "updated_by_discord_user_id")?;
        let updated_at = c.time("updated_at")?;
        let mut versions = BTreeMap::new();
        versions.insert(version, (def.clone(), false, updated_at, updated_by));
        cx.rule_ref.insert(id, id);
        out.push(RuleRow {
            id,
            key,
            name: def.name,
            current_version: version,
            archived_at: c.opt_time("archived_at")?,
            created_at: c.time("created_at")?,
            created_by: actor(cx, &c, "created_by_discord_user_id")?,
            updated_at,
            updated_by,
            rebuilt: false,
            versions,
        });
    }
    Ok(out)
}

fn clean(cx: &mut Ctx<'_>, text: String, column: &str, key: &str) -> String {
    let (text, changed) = convert::clean(text);
    if changed {
        cx.f.normalized(column, key);
    }
    text
}

/// Scans the cases that used a version other than their template's
/// current one (or a template that is gone), in creation order, and
/// rebuilds those versions. Missing templates become archived rules.
async fn scan_snapshots(
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
    fn versions_must_fit_an_integer() {
        assert_eq!(version_of(0), None);
        assert_eq!(version_of(1), Some(1));
        assert_eq!(
            version_of(u64::from(i32::MAX.unsigned_abs())),
            Some(i32::MAX.unsigned_abs())
        );
        assert_eq!(version_of(u64::from(i32::MAX.unsigned_abs()) + 1), None);
    }

    #[test]
    fn derived_ids_stay_version_seven() {
        let id = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
        let d = derived_id(id, 123_456_789);
        assert_ne!(d, id);
        assert_eq!(d.get_version_num(), 7);
        assert_eq!(derived_id(id, 123_456_789), d);
    }
}
