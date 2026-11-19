//! The source manifest (§19.1): every Go table and column the importer
//! accepts, and its fate. Before reading data the importer compares it with
//! `information_schema.columns` and refuses an unknown shape.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sqlx::{MySqlConnection, Row as _};

use crate::ImportError;

/// Bumped whenever an entry below changes; part of the fingerprint.
pub const MANIFEST_VERSION: u32 = 1;

/// The accepted ledger names, in order (§19.1).
pub const LEDGER: [&str; 5] = [
    "baseline",
    "audit_mirror_deliveries",
    "launch_announcement",
    "staff_roles",
    "discord_user_mfa",
];

mod columns;

use self::columns::{A, Fate, Since, TABLES};

/// One source column and its fate, as the report lists it.
#[derive(Clone, Debug, Serialize)]
pub struct ManifestEntry {
    /// Source table.
    pub table: &'static str,
    /// Source column.
    pub column: &'static str,
    /// `mapped` or `dropped`.
    pub fate: &'static str,
    /// The target (mapped) or the reason (dropped).
    pub detail: &'static str,
    /// Whether this source has the column.
    pub present: bool,
}

/// What the source database looks like: its ledger and optional pieces.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    /// The highest ledger version (3, 4, or 5).
    pub ledger: u8,
    /// The ledger rows.
    pub ledger_rows: Vec<(u64, String)>,
    /// Present tables and their columns.
    pub columns: BTreeMap<String, BTreeSet<String>>,
}

impl Shape {
    /// Whether `table` exists.
    pub fn has_table(&self, table: &str) -> bool {
        self.columns.contains_key(table)
    }

    /// Whether `table.column` exists.
    pub fn has(&self, table: &str, column: &str) -> bool {
        self.columns.get(table).is_some_and(|c| c.contains(column))
    }

    /// The optional pieces present, for the report.
    pub fn features(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (table, since, columns) in TABLES {
            if *since != A && self.has_table(table) {
                out.push(format!("table {table}"));
            }
            for (column, since, _) in *columns {
                if *since != A && self.has(table, column) {
                    out.push(format!("{table}.{column}"));
                }
            }
        }
        out
    }

    /// The manifest for this source.
    pub fn manifest(&self) -> Vec<ManifestEntry> {
        let mut out = Vec::new();
        for (table, _, columns) in TABLES {
            for (column, _, fate) in *columns {
                let (fate, detail) = match fate {
                    Fate::To(t) => ("mapped", *t),
                    Fate::Drop(r) => ("dropped", *r),
                };
                out.push(ManifestEntry {
                    table,
                    column,
                    fate,
                    detail,
                    present: self.has(table, column),
                });
            }
        }
        out
    }
}

/// Reads the ledger and `information_schema`, and refuses any shape the
/// manifest does not describe.
pub(crate) async fn inspect(conn: &mut MySqlConnection) -> Result<Shape, ImportError> {
    let rows = sqlx::query(
        "SELECT table_name AS t, column_name AS c FROM information_schema.columns
         WHERE table_schema = DATABASE() ORDER BY table_name, ordinal_position",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(ImportError::source)?;
    let mut columns: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in rows {
        let t: String = row.try_get("t").map_err(ImportError::source)?;
        let c: String = row.try_get("c").map_err(ImportError::source)?;
        columns.entry(t).or_default().insert(c);
    }
    if !columns.contains_key("quack_schema_migrations") {
        return Err(ImportError::Shape(vec![
            "quack_schema_migrations is missing: this is not a Quack database".to_owned(),
        ]));
    }
    let ledger_rows = read_ledger(conn).await?;
    let ledger = check_ledger(&ledger_rows)?;
    let shape = Shape {
        ledger,
        ledger_rows,
        columns,
    };
    check_columns(&shape)?;
    Ok(shape)
}

async fn read_ledger(conn: &mut MySqlConnection) -> Result<Vec<(u64, String)>, ImportError> {
    let rows = sqlx::query("SELECT version, name FROM quack_schema_migrations ORDER BY version")
        .fetch_all(conn)
        .await
        .map_err(ImportError::source)?;
    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<u64, _>("version")
                    .map_err(ImportError::source)?,
                row.try_get::<String, _>("name")
                    .map_err(ImportError::source)?,
            ))
        })
        .collect()
}

/// The ledger must be `1–3`, `1–4`, or `1–5` with the expected names.
pub(crate) fn check_ledger(rows: &[(u64, String)]) -> Result<u8, ImportError> {
    let ok = (3..=5).contains(&rows.len())
        && rows
            .iter()
            .zip(LEDGER)
            .enumerate()
            .all(|(i, ((version, name), want))| *version == i as u64 + 1 && name == want);
    if ok {
        Ok(u8::try_from(rows.len()).unwrap_or(0))
    } else {
        Err(ImportError::Shape(vec![format!(
            "unsupported migration ledger {:?}; expected versions 1–3, 1–4, or 1–5 named {LEDGER:?}",
            rows.iter()
                .map(|(v, n)| format!("{v} {n}"))
                .collect::<Vec<_>>()
        )]))
    }
}

/// Every present table and column must be in the manifest, and every one
/// the ledger guarantees must be present.
pub(crate) fn check_columns(shape: &Shape) -> Result<(), ImportError> {
    let mut problems = Vec::new();
    let required = |since: Since| match since {
        Since::Always => true,
        Since::Ledger(v) => shape.ledger >= v,
        Since::Legacy => false,
    };
    for (table, present) in &shape.columns {
        let Some((_, _, columns)) = TABLES.iter().find(|(t, ..)| t == table) else {
            problems.push(format!("unexpected table {table}"));
            continue;
        };
        for column in present {
            if !columns.iter().any(|(c, ..)| c == column) {
                problems.push(format!("unexpected column {table}.{column}"));
            }
        }
    }
    for (table, since, columns) in TABLES {
        if !shape.has_table(table) {
            if required(*since) {
                problems.push(format!("missing table {table}"));
            }
            continue;
        }
        for (column, since, _) in *columns {
            if required(*since) && !shape.has(table, column) {
                problems.push(format!("missing column {table}.{column}"));
            }
        }
    }
    if !shape.has("appeals", "statement") && !shape.has("appeals", "content") {
        problems.push("appeals has neither statement nor content".to_owned());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ImportError::Shape(problems))
    }
}

/// Every manifest table that exists in the source, in manifest order.
pub(crate) fn present_tables(shape: &Shape) -> Vec<&'static str> {
    TABLES
        .iter()
        .map(|(t, ..)| *t)
        .filter(|t| shape.has_table(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(ledger: u8) -> Shape {
        let mut columns = BTreeMap::new();
        for (table, since, cols) in TABLES {
            let table_present = match since {
                Since::Always => true,
                Since::Ledger(v) => ledger >= *v,
                Since::Legacy => false,
            };
            if !table_present {
                continue;
            }
            let set: BTreeSet<String> = cols
                .iter()
                .filter(|(c, s, _)| match s {
                    Since::Always => true,
                    Since::Ledger(v) => ledger >= *v,
                    Since::Legacy => *c == "statement",
                })
                .map(|(c, ..)| (*c).to_owned())
                .collect();
            columns.insert((*table).to_owned(), set);
        }
        Shape {
            ledger,
            ledger_rows: Vec::new(),
            columns,
        }
    }

    #[test]
    fn every_accepted_ledger_shape_passes() {
        for ledger in 3..=5 {
            check_columns(&shape(ledger)).unwrap();
        }
    }

    #[test]
    fn unexpected_and_missing_columns_abort() {
        let mut s = shape(5);
        s.columns
            .get_mut("cases")
            .unwrap()
            .insert("surprise".into());
        s.columns.get_mut("guilds").unwrap().remove("name");
        let Err(ImportError::Shape(problems)) = check_columns(&s) else {
            panic!("accepted a bad shape");
        };
        assert!(problems.contains(&"unexpected column cases.surprise".to_owned()));
        assert!(problems.contains(&"missing column guilds.name".to_owned()));
        let mut s = shape(5);
        s.columns.remove("discord_user_mfa");
        assert!(check_columns(&s).is_err());
        let mut s = shape(5);
        s.columns.insert("mystery".into(), BTreeSet::new());
        assert!(check_columns(&s).is_err());
    }

    #[test]
    fn legacy_appeal_columns_are_accepted() {
        let mut s = shape(3);
        let appeals = s.columns.get_mut("appeals").unwrap();
        appeals.remove("statement");
        for c in ["content", "answers_json", "question_snapshot_json"] {
            appeals.insert(c.into());
        }
        s.columns.insert(
            "guild_appeal_settings".into(),
            [
                "id",
                "created_at",
                "updated_at",
                "guild_id",
                "questions_json",
                "updated_by_discord_user_id",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        check_columns(&s).unwrap();
        assert!(s.features().contains(&"appeals.content".to_owned()));
    }

    #[test]
    fn ledgers() {
        let rows = |n: usize| -> Vec<(u64, String)> {
            LEDGER
                .iter()
                .take(n)
                .enumerate()
                .map(|(i, n)| (i as u64 + 1, (*n).to_owned()))
                .collect()
        };
        assert_eq!(check_ledger(&rows(3)).unwrap(), 3);
        assert_eq!(check_ledger(&rows(5)).unwrap(), 5);
        assert!(check_ledger(&rows(2)).is_err());
        let mut bad = rows(4);
        bad[3].1 = "other".into();
        assert!(check_ledger(&bad).is_err());
    }
}
