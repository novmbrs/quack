//! Uploaded-files items (§7.1) and the dashboard bytes staged until their
//! copy (§7.3).

use quack_core::ids::{AccountId, CaseId, EvidenceId, FileId, GuildId};
use sqlx::PgConnection;

use super::NewFile;
use super::files::insert_files;
use crate::DbError;
use crate::obs::{self, Delete, Insert, Select};

/// A new uploaded-files item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewUploadItem {
    /// A fresh `UUIDv7`.
    pub id: EvidenceId,
    /// The server.
    pub guild: GuildId,
    /// The case.
    pub case: CaseId,
    /// The staff member who uploaded them.
    pub added_by: AccountId,
    /// The staff note.
    pub note: Option<String>,
    /// Its files (1–10), each `pending`.
    pub files: Vec<NewFile>,
    /// The bytes of files without an original URL (dashboard uploads), by
    /// file (shared buffers, so building the item copies nothing).
    pub staged: Vec<(FileId, bytes::Bytes)>,
}

/// Inserts an uploaded-files item (`saving`, staff-only), its files, and
/// their staged bytes, under the case lock. [`DbError::NotFound`] when the
/// case is not in this server.
pub async fn insert_upload(conn: &mut PgConnection, item: &NewUploadItem) -> Result<(), DbError> {
    let inserted = obs::run(
        Insert,
        "evidence",
        sqlx::query!(
            "INSERT INTO evidence (id, guild_id, case_id, kind, added_by, visible_to_member, note,
                                   note_updated_by, note_updated_at)
             SELECT $1, c.guild_id, c.id, 'upload', $4::bigint, false, $5,
                    CASE WHEN $5::text IS NULL THEN NULL ELSE $4::bigint END,
                    CASE WHEN $5::text IS NULL THEN NULL ELSE now() END
             FROM cases c WHERE c.guild_id = $2 AND c.id = $3",
            item.id.get(),
            i64::from(item.guild),
            item.case.get(),
            i64::from(item.added_by),
            item.note.as_deref(),
        )
        .execute(&mut *conn),
    )
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(DbError::NotFound);
    }
    insert_files(conn, item.guild, item.case, item.id, &item.files).await?;
    for (file, bytes) in &item.staged {
        obs::run(
            Insert,
            "evidence_upload_bytes",
            sqlx::query!(
                "INSERT INTO evidence_upload_bytes (file_id, guild_id, bytes) VALUES ($1, $2, $3)",
                file.get(),
                i64::from(item.guild),
                bytes.as_ref(),
            )
            .execute(&mut *conn),
        )
        .await?;
    }
    Ok(())
}

/// The staged bytes of a dashboard upload, while they are kept.
pub async fn staged_bytes(
    conn: &mut PgConnection,
    guild: GuildId,
    file: FileId,
) -> Result<Option<Vec<u8>>, DbError> {
    obs::run(
        Select,
        "evidence_upload_bytes",
        sqlx::query_scalar!(
            "SELECT bytes FROM evidence_upload_bytes WHERE guild_id = $1 AND file_id = $2",
            i64::from(guild),
            file.get(),
        )
        .fetch_optional(conn),
    )
    .await
}

/// Deletes the staged bytes of one file (`Some`) or of every file of an
/// item (`None`), once copied or settled.
pub async fn delete_staged(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    file: Option<FileId>,
) -> Result<u64, DbError> {
    let done = obs::run(
        Delete,
        "evidence_upload_bytes",
        sqlx::query!(
            "DELETE FROM evidence_upload_bytes b USING evidence_files f
             WHERE b.guild_id = $1 AND f.guild_id = b.guild_id AND f.id = b.file_id
               AND f.evidence_id = $2 AND ($3::uuid IS NULL OR b.file_id = $3)",
            i64::from(guild),
            evidence.get(),
            file.map(FileId::get),
        )
        .execute(conn),
    )
    .await?;
    Ok(done.rows_affected())
}
