//! The validation report (§19.10): what was read, written, skipped, changed,
//! and checked. JSON for machines, [`Report::summary`] for people. It holds
//! IDs and counts only, never moderation content.

mod summary;

use std::collections::BTreeMap;

use serde::Serialize;
use time::OffsetDateTime;

use crate::source::manifest::ManifestEntry;

/// How many example keys each count keeps.
pub const MAX_EXAMPLES: usize = 20;

/// A count with up to [`MAX_EXAMPLES`] example keys (IDs only).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Examples {
    /// How many.
    pub count: u64,
    /// Some of their keys.
    pub examples: Vec<String>,
}

impl Examples {
    /// Counts one more, keeping its key while there is room.
    pub fn add(&mut self, key: impl Into<String>) {
        self.count += 1;
        if self.examples.len() < MAX_EXAMPLES {
            self.examples.push(key.into());
        }
    }

    fn merge(&mut self, other: Self) {
        self.count += other.count;
        for key in other.examples {
            if self.examples.len() < MAX_EXAMPLES {
                self.examples.push(key);
            }
        }
    }
}

/// `table → reason → examples`, and similar two-level maps.
pub type Nested = BTreeMap<String, BTreeMap<String, Examples>>;

/// What was read from MySQL.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SourceSection {
    /// `SELECT VERSION()`.
    pub server_version: String,
    /// The `quack_schema_migrations` rows, `(version, name)`.
    pub ledger: Vec<(u64, String)>,
    /// When the snapshot was taken (MySQL's clock).
    #[serde(with = "time::serde::rfc3339::option")]
    pub snapshot_at: Option<OffsetDateTime>,
    /// The fixed "now" every time comparison uses.
    #[serde(with = "time::serde::rfc3339::option")]
    pub evaluated_at: Option<OffsetDateTime>,
    /// Per table: row count and SHA-256 over every row in key order.
    pub tables: BTreeMap<String, TableDigest>,
    /// SHA-256 over the ledger, manifest and transform versions, and the
    /// table digests (§19.11).
    pub fingerprint: String,
    /// Optional shapes the source has (legacy appeal columns, …).
    pub shape: Vec<String>,
    /// Every source column and its fate.
    pub manifest: Vec<ManifestEntry>,
}

/// One table's row count and digest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TableDigest {
    /// Rows.
    pub rows: u64,
    /// SHA-256, hex.
    pub sha256: String,
}

/// One relationship check.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Check {
    /// Whether it held everywhere.
    pub ok: bool,
    /// The keys where it did not.
    pub failing: Examples,
}

/// One renamed rule (§19.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RenamedRule {
    /// The rule's UUID.
    pub rule_id: String,
    /// Its name in MySQL.
    pub old_name: String,
    /// Its name now.
    pub new_name: String,
}

/// One rebuilt rule version (§19.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReconstructedVersion {
    /// The rule's UUID.
    pub rule_id: String,
    /// The version number.
    pub version: u32,
    /// How many levels the cases showed.
    pub known_levels: usize,
}

/// What happened to one server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuildOutcome {
    /// Written and committed.
    Imported,
    /// Written and rolled back (dry run), every constraint checked.
    Checked,
    /// Already imported from this same source; nothing done.
    AlreadyImported,
    /// Aborted; see `problems`.
    Failed,
}

/// The import report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    /// `import_runs.id`.
    pub run_id: Option<String>,
    /// `dry_run` or `apply`.
    pub mode: String,
    /// The transform version (§19.11).
    pub transform_version: i32,
    /// Overall: every server imported (or checked) and every check passed.
    pub ok: bool,
    /// What was read.
    pub source: SourceSection,
    /// Per server: its outcome.
    pub guilds: BTreeMap<String, GuildOutcome>,
    /// `table → server → rows` written (or that would be, in a dry run).
    pub written: Nested,
    /// `table → reason → examples` not imported.
    pub skipped: Nested,
    /// Rows constraint checks refused, quarantined instead of written:
    /// `table → constraint → keys`.
    pub quarantined: Nested,
    /// `table.column → examples` over the new limit, imported unchanged.
    pub over_limit: BTreeMap<String, Examples>,
    /// `table.column → examples` whose invalid characters were replaced.
    pub normalized: BTreeMap<String, Examples>,
    /// `code → examples` the importer had to derive.
    pub derived: BTreeMap<String, Examples>,
    /// Rules renamed for name uniqueness.
    pub renamed_rules: Vec<RenamedRule>,
    /// Versions rebuilt from case snapshots.
    pub reconstructed_versions: Vec<ReconstructedVersion>,
    /// Cases whose snapshots disagreed with an earlier case's.
    pub conflicting_snapshots: Examples,
    /// Rules whose default level was replaced by a level at case count 1.
    pub dropped_default_levels: Examples,
    /// Imported moderation log entries by Go action name.
    pub legacy_mod_log: BTreeMap<String, u64>,
    /// Moderation log links left NULL, per link column.
    pub legacy_mod_log_null_links: BTreeMap<String, Examples>,
    /// Counts per row of §19.9.
    pub in_flight: BTreeMap<String, Examples>,
    /// Tables not imported: `table → category → examples`.
    pub not_imported: Nested,
    /// Relationship checks.
    pub checks: BTreeMap<String, Check>,
    /// Errors that aborted a server (or the run, under `run`).
    pub problems: BTreeMap<String, Vec<String>>,
}

/// The part of the report one server's transaction produces. Merged into
/// the [`Report`] only if the server's transaction succeeds, so a rolled
/// back server leaves nothing but its problems.
#[derive(Debug, Default)]
pub struct Fragment {
    pub(crate) written: BTreeMap<String, u64>,
    pub(crate) skipped: Nested,
    pub(crate) quarantined: Nested,
    pub(crate) over_limit: BTreeMap<String, Examples>,
    pub(crate) normalized: BTreeMap<String, Examples>,
    pub(crate) derived: BTreeMap<String, Examples>,
    pub(crate) renamed_rules: Vec<RenamedRule>,
    pub(crate) reconstructed_versions: Vec<ReconstructedVersion>,
    pub(crate) conflicting_snapshots: Examples,
    pub(crate) dropped_default_levels: Examples,
    pub(crate) legacy_mod_log: BTreeMap<String, u64>,
    pub(crate) null_links: BTreeMap<String, Examples>,
    pub(crate) in_flight: BTreeMap<String, Examples>,
    pub(crate) checks: BTreeMap<String, Check>,
}

impl Fragment {
    pub(crate) fn wrote(&mut self, table: &str) {
        *self.written.entry(table.to_owned()).or_default() += 1;
    }

    pub(crate) fn skip(&mut self, table: &str, reason: &str, key: impl Into<String>) {
        nested(&mut self.skipped, table, reason).add(key);
    }

    pub(crate) fn quarantine(&mut self, table: &str, constraint: &str, key: impl Into<String>) {
        nested(&mut self.quarantined, table, constraint).add(key);
    }

    pub(crate) fn over_limit(&mut self, column: &str, key: impl Into<String>) {
        self.over_limit
            .entry(column.to_owned())
            .or_default()
            .add(key);
    }

    pub(crate) fn normalized(&mut self, column: &str, key: impl Into<String>) {
        self.normalized
            .entry(column.to_owned())
            .or_default()
            .add(key);
    }

    pub(crate) fn derived(&mut self, code: &str, key: impl Into<String>) {
        self.derived.entry(code.to_owned()).or_default().add(key);
    }

    pub(crate) fn in_flight(&mut self, row: &str, key: impl Into<String>) {
        self.in_flight.entry(row.to_owned()).or_default().add(key);
    }

    pub(crate) fn null_link(&mut self, column: &str, key: impl Into<String>) {
        self.null_links
            .entry(column.to_owned())
            .or_default()
            .add(key);
    }

    pub(crate) fn check(&mut self, name: &str, failing: Vec<String>) {
        let check = self.checks.entry(name.to_owned()).or_insert(Check {
            ok: true,
            failing: Examples::default(),
        });
        for key in failing {
            check.ok = false;
            check.failing.add(key);
        }
    }
}

pub(crate) fn nested<'a>(map: &'a mut Nested, outer: &str, inner: &str) -> &'a mut Examples {
    map.entry(outer.to_owned())
        .or_default()
        .entry(inner.to_owned())
        .or_default()
}

fn merge_nested(into: &mut Nested, from: Nested) {
    for (outer, inner) in from {
        for (key, examples) in inner {
            nested(into, &outer, &key).merge(examples);
        }
    }
}

fn merge_flat(into: &mut BTreeMap<String, Examples>, from: BTreeMap<String, Examples>) {
    for (key, examples) in from {
        into.entry(key).or_default().merge(examples);
    }
}

impl Report {
    /// Adds a committed (or dry-run checked) server's fragment.
    pub(crate) fn merge(&mut self, guild: &str, fragment: Fragment) {
        for (table, rows) in fragment.written {
            self.written
                .entry(table)
                .or_default()
                .entry(guild.to_owned())
                .or_default()
                .count += rows;
        }
        merge_nested(&mut self.skipped, fragment.skipped);
        merge_nested(&mut self.quarantined, fragment.quarantined);
        merge_flat(&mut self.over_limit, fragment.over_limit);
        merge_flat(&mut self.normalized, fragment.normalized);
        merge_flat(&mut self.derived, fragment.derived);
        merge_flat(&mut self.in_flight, fragment.in_flight);
        merge_flat(&mut self.legacy_mod_log_null_links, fragment.null_links);
        self.renamed_rules.extend(fragment.renamed_rules);
        self.reconstructed_versions
            .extend(fragment.reconstructed_versions);
        self.conflicting_snapshots
            .merge(fragment.conflicting_snapshots);
        self.dropped_default_levels
            .merge(fragment.dropped_default_levels);
        for (action, n) in fragment.legacy_mod_log {
            *self.legacy_mod_log.entry(action).or_default() += n;
        }
        for (name, check) in fragment.checks {
            self.add_check(&name, check);
        }
    }

    pub(crate) fn add_check(&mut self, name: &str, check: Check) {
        let entry = self.checks.entry(name.to_owned()).or_insert(Check {
            ok: true,
            failing: Examples::default(),
        });
        entry.ok &= check.ok;
        entry.failing.merge(check.failing);
    }

    pub(crate) fn problem(&mut self, scope: &str, problem: String) {
        self.problems
            .entry(scope.to_owned())
            .or_default()
            .push(problem);
    }

    /// Sets `ok` from the outcomes, checks, and problems.
    pub(crate) fn finish(&mut self) {
        self.ok = self.problems.is_empty()
            && self.checks.values().all(|c| c.ok)
            && self.guilds.values().all(|g| *g != GuildOutcome::Failed);
    }

    /// Total rows written to `table` across servers.
    pub fn written_total(&self, table: &str) -> u64 {
        self.written
            .get(table)
            .map_or(0, |per| per.values().map(|e| e.count).sum())
    }

    /// The report as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_cap_at_twenty() {
        let mut e = Examples::default();
        for i in 0..25 {
            e.add(i.to_string());
        }
        assert_eq!(e.count, 25);
        assert_eq!(e.examples.len(), MAX_EXAMPLES);
    }

    #[test]
    fn merging_a_fragment_sums_counts() {
        let mut report = Report::default();
        let mut f = Fragment::default();
        f.wrote("cases");
        f.wrote("cases");
        f.skip("cases", "invalid_snowflake", "a");
        f.check("counter", vec!["g".into()]);
        report.merge("1", f);
        assert_eq!(report.written_total("cases"), 2);
        assert_eq!(report.skipped["cases"]["invalid_snowflake"].count, 1);
        report.finish();
        assert!(!report.ok);
        assert!(report.summary().contains("counter: FAILED"));
    }
}
