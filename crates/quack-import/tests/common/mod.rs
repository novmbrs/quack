//! Scratch databases for the end-to-end tests.
//!
//! The tests need a MySQL 8.4 server and a PostgreSQL 18 server they may
//! create databases on:
//!
//! - `QUACK_IMPORT_TEST_MYSQL_URL`, e.g. `mysql://root:root@127.0.0.1:3307`
//! - `QUACK_IMPORT_TEST_PG_URL`, e.g. `postgres://postgres:pg@127.0.0.1:5433/postgres`
//!
//! Without them every test prints `SKIPPED` and passes (it fails when `CI`
//! is set), so a plain
//! `cargo test --workspace` works anywhere. Each test creates its own pair
//! of databases and drops them at the end.

#![allow(dead_code, unreachable_pub, missing_docs)]

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context as _, Result};
use quack_db::Db;
use quack_import::{ImportOptions, Report};
use sqlx::{Connection as _, MySqlConnection, PgConnection};

static NEXT: AtomicU32 = AtomicU32::new(0);

pub const SCHEMA_V3: &str = include_str!("../fixtures/schema_v3.sql");
pub const DATA: &str = include_str!("../fixtures/data.sql");
pub const LEDGER_V3: &str = include_str!("../fixtures/ledger_v3.sql");
pub const UPGRADE_V5: &str = include_str!("../fixtures/upgrade_v5.sql");
pub const LEGACY_APPEALS: &str = include_str!("../fixtures/legacy_appeals.sql");
pub const BROKEN: &str = include_str!("../fixtures/broken.sql");

/// A MySQL source database and a migrated PostgreSQL target, both new.
pub struct Scratch {
    name: String,
    mysql_admin: String,
    pg_admin: String,
    pub source_url: String,
    pub db: Db,
}

/// The server URLs, or `None` (and a loud note on stderr) when they are
/// unset. In CI (`CI` set) a skip fails the test instead, so the importer's
/// end-to-end tests can't pass there without running.
pub fn env() -> Option<(String, String)> {
    if let (Ok(m), Ok(p)) = (
        std::env::var("QUACK_IMPORT_TEST_MYSQL_URL"),
        std::env::var("QUACK_IMPORT_TEST_PG_URL"),
    ) {
        Some((m, p))
    } else {
        let note = "set QUACK_IMPORT_TEST_MYSQL_URL and QUACK_IMPORT_TEST_PG_URL to run the importer's end-to-end tests";
        assert!(
            std::env::var_os("CI").is_none(),
            "not skipped in CI: {note}"
        );
        eprintln!("SKIPPED (nothing was checked): {note}");
        None
    }
}

/// `url` with its database replaced by `db` (query kept).
fn with_db(url: &str, db: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?']).next().unwrap_or(rest);
    let query = rest.find('?').map_or("", |i| &rest[i..]);
    format!("{scheme}://{host}/{db}{query}")
}

/// Runs a fixture: statements end with `;` at the end of a line.
pub async fn load(conn: &mut MySqlConnection, sql: &str) -> Result<()> {
    let body: String = sql
        .lines()
        .filter(|l| !l.starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    for statement in body.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
        let statement = statement.trim_end_matches(';');
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&mut *conn)
            .await
            .with_context(|| {
                format!(
                    "fixture statement failed: {}",
                    &statement[..statement.len().min(120)]
                )
            })?;
    }
    Ok(())
}

impl Scratch {
    /// New databases with `fixtures` loaded into MySQL, in order.
    pub async fn new(fixtures: &[&str]) -> Result<Option<Self>> {
        let Some((mysql_admin, pg_admin)) = env() else {
            return Ok(None);
        };
        let name = format!(
            "qi_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let mut admin = MySqlConnection::connect(&mysql_admin).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE `{name}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
        )))
        .execute(&mut admin)
        .await?;
        let source_url = with_db(&mysql_admin, &name);
        let mut conn = MySqlConnection::connect(&source_url).await?;
        for fixture in fixtures {
            load(&mut conn, fixture).await?;
        }
        let mut pg = PgConnection::connect(&pg_admin).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&mut pg)
            .await?;
        let db = Db::connect(&with_db(&pg_admin, &name), 8).await?;
        quack_db::migrate(&db).await?;
        Ok(Some(Self {
            name,
            mysql_admin,
            pg_admin,
            source_url,
            db,
        }))
    }

    /// Runs more SQL against the MySQL source.
    pub async fn mysql(&self, sql: &str) -> Result<()> {
        let mut conn = MySqlConnection::connect(&self.source_url).await?;
        load(&mut conn, sql).await
    }

    /// Imports with the given mode.
    pub async fn import(&self, dry_run: bool) -> Result<Report, quack_import::ImportError> {
        quack_import::run(
            &self.db,
            &ImportOptions {
                source_url: self.source_url.clone(),
                dry_run,
                report_path: None,
            },
        )
        .await
    }

    /// One integer from PostgreSQL.
    pub async fn count(&self, sql: &str) -> Result<i64> {
        let sql = format!("SELECT ({sql})::bigint");
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(self.db.pool())
            .await?)
    }

    /// One text value (or NULL) from PostgreSQL.
    pub async fn text(&self, sql: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(self.db.pool())
            .await?)
    }

    /// Row counts of every table the importer writes, for comparing runs.
    pub async fn counts(&self) -> Result<Vec<(String, i64)>> {
        let mut out = Vec::new();
        for table in TABLES {
            out.push((
                (*table).to_owned(),
                self.count(&format!("SELECT count(*) FROM {table}")).await?,
            ));
        }
        Ok(out)
    }

    /// Drops both databases.
    pub async fn drop(self) -> Result<()> {
        self.db.close().await;
        let mut admin = MySqlConnection::connect(&self.mysql_admin).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE `{}`",
            self.name
        )))
        .execute(&mut admin)
        .await?;
        let mut pg = PgConnection::connect(&self.pg_admin).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{}\" WITH (FORCE)",
            self.name
        )))
        .execute(&mut pg)
        .await?;
        Ok(())
    }
}

/// Every table the importer writes.
pub const TABLES: &[&str] = &[
    "guilds",
    "case_counters",
    "discord_accounts",
    "rules",
    "rule_versions",
    "guild_settings",
    "ticket_settings",
    "activity_log_settings",
    "activity_log_routes",
    "honeypot_settings",
    "cases",
    "actions",
    "action_attempts",
    "evidence",
    "evidence_files",
    "appeals",
    "appeal_events",
    "tickets",
    "ticket_counters",
    "ticket_messages",
    "ticket_transcripts",
    "dms",
    "mod_log",
    "messages",
    "jobs",
    "honeypot_hits",
    "import_guilds",
];

/// Server A of the fixture.
pub const A: &str = "100000000000000001";
/// Server B of the fixture.
pub const B: &str = "100000000000000002";

/// A fixture ULID (as `gen_fixture.py` writes them).
pub fn ulid(kind: &str, n: u32) -> String {
    format!("01JA{kind}{n:04}0000000000000000")
}

/// The UUID a fixture ULID becomes.
pub fn id(kind: &str, n: u32) -> String {
    quack_core::ids::ulid_to_uuid(&ulid(kind, n))
        .unwrap_or_else(|_| unreachable!("fixture IDs are ULIDs"))
        .to_string()
}

/// The Go schema at ledger version 5 with the fixture data.
pub async fn v5() -> Result<Option<Scratch>> {
    Scratch::new(&[SCHEMA_V3, DATA, LEDGER_V3, UPGRADE_V5]).await
}
