//! `case_templates` and their children → `rules` + immutable
//! `rule_versions` (§19.5): the current version from the child rows, older
//! versions rebuilt from the snapshots of the cases that used them.

pub(crate) mod current;
mod names;
mod rebuild;
pub(crate) mod snapshot;

use std::collections::{BTreeMap, HashMap};

use quack_core::rule::{RuleDefinition, Strictness};
use time::OffsetDateTime;
use uuid::Uuid;

use self::current::{FieldRow, LevelRow, Template};
use self::rebuild::scan_snapshots;
use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::report::ReconstructedVersion;
use crate::source::Cols;
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
pub(super) fn version_of(raw: u64) -> Option<u32> {
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
}
