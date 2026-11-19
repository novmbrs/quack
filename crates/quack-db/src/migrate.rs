//! Schema migrations: applied by `quack migrate`, verified by `quack serve`.

use sqlx::Connection as _;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgConnection;

use crate::{Db, DbError};

/// The migrations compiled into this binary (`crates/quack-db/migrations`).
/// Ledger table: `_sqlx_migrations`.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// One migration this binary knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationInfo {
    /// The version (the file name's number).
    pub version: i64,
    /// The description from the file name.
    pub description: String,
}

/// Applies pending migrations on a dedicated connection (no statement
/// timeout, so long DDL can finish) and returns every migration the binary
/// knows, in order. Concurrent runners wait on sqlx's migration lock.
pub async fn migrate(db: &Db) -> Result<Vec<MigrationInfo>, DbError> {
    let mut conn = PgConnection::connect_with(db.connect_options()).await?;
    MIGRATOR.run(&mut conn).await?;
    conn.close().await?;
    Ok(MIGRATOR
        .iter()
        .map(|m| MigrationInfo {
            version: m.version,
            description: m.description.to_string(),
        })
        .collect())
}

/// Why the applied migrations don't match this binary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MigrationMismatch {
    /// The database has never been migrated.
    #[error("the database has no migrations; run `quack migrate`")]
    NotMigrated,
    /// These versions are known to the binary but not applied.
    #[error("migrations {0:?} are not applied; run `quack migrate`")]
    Pending(Vec<i64>),
    /// These versions are applied but unknown to the binary (a newer binary
    /// migrated the database).
    #[error("migrations {0:?} are applied but unknown to this binary")]
    Unknown(Vec<i64>),
    /// An applied migration's file changed after it was applied, or it
    /// failed half-way.
    #[error("migration {0} differs from the applied one or failed")]
    Changed(i64),
}

/// Checks cheaply that the applied migrations are exactly this binary's
/// set, with matching checksums. `Ok(Err(_))` says what differs.
pub async fn verify_migrations(db: &Db) -> Result<Result<(), MigrationMismatch>, DbError> {
    let mut conn = db.conn().await?;
    let exists = crate::obs::run(
        crate::obs::Select,
        "_sqlx_migrations",
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *conn),
    )
    .await?;
    if !exists {
        return Ok(Err(MigrationMismatch::NotMigrated));
    }
    let applied: Vec<(i64, Vec<u8>, bool)> = crate::obs::run(
        crate::obs::Select,
        "_sqlx_migrations",
        sqlx::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn),
    )
    .await?;
    Ok(compare(&applied))
}

fn compare(applied: &[(i64, Vec<u8>, bool)]) -> Result<(), MigrationMismatch> {
    if applied.is_empty() {
        return Err(MigrationMismatch::NotMigrated);
    }
    let unknown: Vec<i64> = applied
        .iter()
        .map(|(v, ..)| *v)
        .filter(|v| !MIGRATOR.iter().any(|m| m.version == *v))
        .collect();
    if !unknown.is_empty() {
        return Err(MigrationMismatch::Unknown(unknown));
    }
    let mut pending = Vec::new();
    for migration in MIGRATOR.iter() {
        match applied.iter().find(|(v, ..)| *v == migration.version) {
            None => pending.push(migration.version),
            Some((_, checksum, success)) => {
                if !success || checksum.as_slice() != &*migration.checksum {
                    return Err(MigrationMismatch::Changed(migration.version));
                }
            }
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        Err(MigrationMismatch::Pending(pending))
    }
}
