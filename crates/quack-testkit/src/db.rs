//! A fresh, migrated PostgreSQL database per test.
//!
//! The server comes from `QUACK_TEST_DATABASE_URL`, which must be set: the
//! tests create and drop databases there, so they never pick a server on
//! their own (another project's on a common port, say). `docker compose up
//! -d` starts one at `postgres://quack:quack@127.0.0.1:54418/quack`; any
//! PostgreSQL 18 works, for example a throwaway container:
//!
//! ```text
//! docker run -d --name quack-test-pg -e POSTGRES_USER=quack -e POSTGRES_PASSWORD=quack \
//!   -e POSTGRES_DB=quack -p 127.0.0.1:55871:5432 postgres:18-alpine -c max_connections=400
//! QUACK_TEST_DATABASE_URL=postgres://quack:quack@127.0.0.1:55871/quack cargo test -p quack-engine
//! ```
//!
//! The migrations are applied once into a template database (named after
//! their checksums); each test database is a copy of it, dropped when the
//! `TestDb` is dropped.

use std::str::FromStr as _;

use anyhow::{Context as _, Result};
use quack_db::Db;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection as _, PgConnection};

/// The variable naming the test server.
pub const URL_VAR: &str = "QUACK_TEST_DATABASE_URL";

/// A migrated database of its own, dropped on drop.
#[derive(Debug)]
pub struct TestDb {
    /// The pool on it.
    pub db: Db,
    name: String,
    admin: PgConnectOptions,
}

impl TestDb {
    /// A fresh database with a pool of 16 connections.
    pub async fn new() -> Result<Self> {
        Self::with_connections(16).await
    }

    /// A fresh database with a pool of `connections` connections.
    pub async fn with_connections(connections: u32) -> Result<Self> {
        let admin = admin_options()?;
        let template = template(&admin).await?;
        let name = format!("quack_test_{}", uuid::Uuid::now_v7().simple());
        let mut conn = PgConnection::connect_with(&admin).await?;
        sqlx::query(AssertSqlSafe(format!(
            "CREATE DATABASE {name} TEMPLATE {template}"
        )))
        .execute(&mut conn)
        .await
        .context("create the test database")?;
        conn.close().await?;
        let db = Db::connect_with(admin.clone().database(&name), connections).await?;
        Ok(Self { db, name, admin })
    }

    /// Runs one raw statement (test set-up and time travel only).
    pub async fn execute(&self, sql: &str) -> Result<u64> {
        let mut conn = self.db.conn().await?;
        Ok(sqlx::query(AssertSqlSafe(sql.to_owned()))
            .execute(&mut *conn)
            .await?
            .rows_affected())
    }

    /// Runs one raw count query.
    pub async fn count(&self, sql: &str) -> Result<i64> {
        let mut conn = self.db.conn().await?;
        Ok(sqlx::query_scalar(AssertSqlSafe(sql.to_owned()))
            .fetch_one(&mut *conn)
            .await?)
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let name = self.name.clone();
        let admin = self.admin.clone();
        // The test's runtime may be shutting down; drop on a thread of its own.
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                if let Ok(mut conn) = PgConnection::connect_with(&admin).await {
                    let _ = sqlx::query(AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                    )))
                    .execute(&mut conn)
                    .await;
                    let _ = conn.close().await;
                }
            });
        })
        .join();
    }
}

/// The test server's URL from [`URL_VAR`], or a clear error when it is not
/// set.
pub fn server_url() -> Result<String> {
    url_from(std::env::var(URL_VAR).ok())
}

fn url_from(value: Option<String>) -> Result<String> {
    match value {
        Some(url) if !url.trim().is_empty() => Ok(url),
        _ => anyhow::bail!(
            "{URL_VAR} is not set. Database tests create and drop databases on the PostgreSQL 18 \
             server it names; start one with `docker compose up -d` and set \
             {URL_VAR}=postgres://quack:quack@127.0.0.1:54418/quack (see AGENTS.md)"
        ),
    }
}

fn admin_options() -> Result<PgConnectOptions> {
    PgConnectOptions::from_str(&server_url()?)
        .with_context(|| format!("{URL_VAR} is not a PostgreSQL URL"))
}

/// The migrated template database, created once (under an advisory lock,
/// so parallel test processes don't race).
async fn template(admin: &PgConnectOptions) -> Result<String> {
    let mut fingerprint: u64 = 0xcbf2_9ce4_8422_2325;
    for migration in quack_db::MIGRATOR.iter() {
        for byte in migration.checksum.iter() {
            fingerprint ^= u64::from(*byte);
            fingerprint = fingerprint.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let name = format!("quack_tpl_{fingerprint:016x}");
    let mut conn = PgConnection::connect_with(admin).await?;
    sqlx::query("SELECT pg_advisory_lock(7177)")
        .execute(&mut conn)
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&name)
            .fetch_one(&mut conn)
            .await?;
    if !exists {
        let building = format!("{name}_building");
        sqlx::query(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {building} WITH (FORCE)"
        )))
        .execute(&mut conn)
        .await?;
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {building}")))
            .execute(&mut conn)
            .await?;
        let db = Db::connect_with(admin.clone().database(&building), 2).await?;
        quack_db::migrate(&db).await?;
        db.close().await;
        sqlx::query(AssertSqlSafe(format!(
            "ALTER DATABASE {building} RENAME TO {name}"
        )))
        .execute(&mut conn)
        .await?;
    }
    sqlx::query("SELECT pg_advisory_unlock(7177)")
        .execute(&mut conn)
        .await?;
    conn.close().await?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::url_from;

    #[test]
    fn an_unset_server_is_an_error_not_a_guess() {
        for unset in [None, Some(String::new())] {
            let message = format!("{:#}", url_from(unset).unwrap_err());
            assert!(
                message.contains("QUACK_TEST_DATABASE_URL is not set"),
                "{message}"
            );
        }
        assert_eq!(
            url_from(Some("postgres://h/db".to_owned())).unwrap(),
            "postgres://h/db"
        );
    }
}
