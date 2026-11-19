//! The human summary of a [`Report`]: the JSON's sections, counts only.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::{Examples, Nested, Report};

impl Report {
    /// The human summary: the same sections as the JSON, counts only.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let verdict = if self.ok { "OK" } else { "NOT OK" };
        let _ = writeln!(out, "Quack MySQL import ({}) — {verdict}", self.mode);
        let _ = writeln!(
            out,
            "source: MySQL {}, ledger {:?}, fingerprint {}",
            self.source.server_version,
            self.source
                .ledger
                .iter()
                .map(|(v, _)| *v)
                .collect::<Vec<_>>(),
            short(&self.source.fingerprint)
        );
        if !self.source.shape.is_empty() {
            let _ = writeln!(out, "shape: {}", self.source.shape.join(", "));
        }
        let _ = writeln!(out, "\nservers:");
        for (guild, outcome) in &self.guilds {
            let _ = writeln!(out, "  {guild}: {outcome:?}");
        }
        let _ = writeln!(out, "\ntables (source rows → written):");
        for (table, digest) in &self.source.tables {
            let _ = writeln!(out, "  {table}: {}", digest.rows);
        }
        for (table, per) in &self.written {
            let total: u64 = per.values().map(|e| e.count).sum();
            let _ = writeln!(out, "  → {table}: {total}");
        }
        section_nested(&mut out, "skipped", &self.skipped);
        section_nested(&mut out, "quarantined", &self.quarantined);
        section_flat(
            &mut out,
            "over limit (imported unchanged)",
            &self.over_limit,
        );
        section_flat(&mut out, "normalized", &self.normalized);
        section_flat(&mut out, "derived", &self.derived);
        section_flat(&mut out, "in-flight work", &self.in_flight);
        section_nested(&mut out, "not imported", &self.not_imported);
        let _ = writeln!(
            out,
            "\nrules: {} renamed, {} versions reconstructed, {} conflicting snapshots, {} default levels dropped",
            self.renamed_rules.len(),
            self.reconstructed_versions.len(),
            self.conflicting_snapshots.count,
            self.dropped_default_levels.count
        );
        let entries: u64 = self.legacy_mod_log.values().sum();
        let _ = writeln!(
            out,
            "moderation log: {entries} legacy entries in {} actions",
            self.legacy_mod_log.len()
        );
        section_flat(
            &mut out,
            "moderation log links left NULL",
            &self.legacy_mod_log_null_links,
        );
        let _ = writeln!(out, "\nchecks:");
        for (name, check) in &self.checks {
            if check.ok {
                let _ = writeln!(out, "  {name}: ok");
            } else {
                let _ = writeln!(out, "  {name}: FAILED {}", examples(&check.failing));
            }
        }
        if !self.problems.is_empty() {
            let _ = writeln!(out, "\nproblems:");
            for (scope, problems) in &self.problems {
                for problem in problems {
                    let _ = writeln!(out, "  {scope}: {problem}");
                }
            }
        }
        out
    }
}

fn short(s: &str) -> &str {
    s.get(..16).unwrap_or(s)
}

fn examples(e: &Examples) -> String {
    if e.examples.is_empty() {
        e.count.to_string()
    } else {
        format!(
            "{} (e.g. {})",
            e.count,
            e.examples
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn section_flat(out: &mut String, title: &str, map: &BTreeMap<String, Examples>) {
    if map.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title}:");
    for (key, e) in map {
        let _ = writeln!(out, "  {key}: {}", examples(e));
    }
}

fn section_nested(out: &mut String, title: &str, map: &Nested) {
    if map.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{title}:");
    for (outer, inner) in map {
        for (key, e) in inner {
            let _ = writeln!(out, "  {outer} / {key}: {}", examples(e));
        }
    }
}
