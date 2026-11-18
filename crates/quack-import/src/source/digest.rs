//! Per-table row counts and digests, and the source fingerprint (§19.11).
//!
//! Each row is hashed inside MySQL (SHA-256 over every column's hex, with
//! NULL marked) and the hashes are folded in primary-key order, so any
//! changed, added, or removed row changes the fingerprint.

use std::collections::BTreeMap;

use aws_lc_rs::digest::{Context, SHA256};
use tracing::Instrument as _;
// Dynamic SQL here only splices manifest table and column names, which
// were checked against information_schema; values are bound.
use sqlx::{AssertSqlSafe, Row as _};

use super::Source;
use super::manifest::{self, MANIFEST_VERSION};
use crate::report::TableDigest;
use crate::{ImportError, TRANSFORM_VERSION};

/// Rows per digest page.
const DIGEST_PAGE: usize = 5_000;

/// Counts and digests every present source table.
pub(crate) async fn tables(
    source: &mut Source,
) -> Result<BTreeMap<String, TableDigest>, ImportError> {
    let mut out = BTreeMap::new();
    for table in manifest::present_tables(&source.shape) {
        let span = tracing::info_span!("import digest", import.table = table);
        let digest = table_digest(source, table).instrument(span).await?;
        tracing::info!(
            import.table = table,
            import.rows = digest.rows,
            "digested source table"
        );
        out.insert(table.to_owned(), digest);
    }
    Ok(out)
}

async fn table_digest(source: &mut Source, table: &str) -> Result<TableDigest, ImportError> {
    let columns: Vec<String> = source
        .shape
        .columns
        .get(table)
        .map_or_else(Vec::new, |c| c.iter().cloned().collect());
    let keys = primary_key(source, table).await?;
    let parts: Vec<String> = columns
        .iter()
        .map(|c| format!("IF(`{c}` IS NULL, 'N', CONCAT('V', HEX(`{c}`)))"))
        .collect();
    // A total order every page agrees on: the key's bytes.
    let key = if let [one] = keys.as_slice() {
        format!("BINARY `{one}`")
    } else {
        let cols: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
        format!("BINARY CONCAT_WS(0x1f, {})", cols.join(", "))
    };
    let select = format!(
        "SELECT {key} AS k, SHA2(CONCAT_WS(',', {}), 256) AS h FROM `{table}`",
        parts.join(", ")
    );
    let mut ctx = Context::new(&SHA256);
    let mut count = 0u64;
    let mut after: Option<Vec<u8>> = None;
    loop {
        let rows = match &after {
            None => {
                sqlx::query(AssertSqlSafe(format!(
                    "{select} ORDER BY {key} LIMIT {DIGEST_PAGE}"
                )))
                .fetch_all(source.conn())
                .await
            }
            Some(last) => {
                sqlx::query(AssertSqlSafe(format!(
                    "{select} WHERE {key} > ? ORDER BY {key} LIMIT {DIGEST_PAGE}"
                )))
                .bind(last.clone())
                .fetch_all(source.conn())
                .await
            }
        }
        .map_err(ImportError::source)?;
        let Some(last) = rows.last() else { break };
        after = Some(
            last.try_get::<Vec<u8>, _>("k")
                .map_err(ImportError::source)?,
        );
        for row in &rows {
            let h: String = row.try_get("h").map_err(ImportError::source)?;
            ctx.update(h.as_bytes());
            count += 1;
        }
        if rows.len() < DIGEST_PAGE {
            break;
        }
    }
    Ok(TableDigest {
        rows: count,
        sha256: hex(ctx.finish().as_ref()),
    })
}

async fn primary_key(source: &mut Source, table: &str) -> Result<Vec<String>, ImportError> {
    let rows = source
        .rows(
            "SELECT column_name AS c FROM information_schema.key_column_usage
             WHERE table_schema = DATABASE() AND table_name = ? AND constraint_name = 'PRIMARY'
             ORDER BY ordinal_position",
            &[table],
        )
        .await?;
    let keys: Vec<String> = rows
        .iter()
        .map(|r| r.try_get::<String, _>("c"))
        .collect::<Result<_, _>>()
        .map_err(ImportError::source)?;
    if keys.is_empty() {
        // No primary key: order by every column so the digest is stable.
        Ok(source
            .shape
            .columns
            .get(table)
            .map_or_else(Vec::new, |c| c.iter().cloned().collect()))
    } else {
        Ok(keys)
    }
}

/// SHA-256 over the ledger, the manifest and transform versions, and every
/// table's count and digest.
pub(crate) fn fingerprint(
    ledger: &[(u64, String)],
    tables: &BTreeMap<String, TableDigest>,
) -> String {
    let mut ctx = Context::new(&SHA256);
    for (version, name) in ledger {
        ctx.update(format!("ledger {version} {name}\n").as_bytes());
    }
    ctx.update(format!("manifest {MANIFEST_VERSION}\ntransform {TRANSFORM_VERSION}\n").as_bytes());
    for (table, digest) in tables {
        ctx.update(format!("table {table} {} {}\n", digest.rows, digest.sha256).as_bytes());
    }
    hex(ctx.finish().as_ref())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_any_input() {
        let ledger = vec![(1, "baseline".to_owned())];
        let mut tables = BTreeMap::new();
        tables.insert(
            "cases".to_owned(),
            TableDigest {
                rows: 1,
                sha256: "aa".into(),
            },
        );
        let a = fingerprint(&ledger, &tables);
        assert_eq!(a.len(), 64);
        assert_eq!(a, fingerprint(&ledger, &tables));
        tables.get_mut("cases").unwrap().sha256 = "ab".into();
        assert_ne!(a, fingerprint(&ledger, &tables));
        assert_ne!(a, fingerprint(&[], &BTreeMap::new()));
    }
}
