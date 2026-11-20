//! Staff edits of an evidence item (§7.5): the note, the member's view of
//! an upload, and removal. The caller holds the case and item locks and has
//! checked the item allows the edit; each write is guarded on the state it
//! read.

use quack_core::ids::{AccountId, EvidenceId, GuildId};
use sqlx::PgConnection;

use crate::DbError;
use crate::obs::{self, Update};

/// Whether an edit changed the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edited {
    /// It changed.
    Changed,
    /// It already was so (or no longer matched the guard).
    Unchanged,
}

impl Edited {
    const fn of(rows: u64) -> Self {
        if rows == 1 {
            Self::Changed
        } else {
            Self::Unchanged
        }
    }
}

/// Sets the note (`None` clears it) while it is still at `revision`,
/// bumping the revision. Setting the same note is `Unchanged`.
pub async fn set_note(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    note: Option<&str>,
    revision: u32,
    by: AccountId,
) -> Result<Edited, DbError> {
    let done = obs::run(
        Update,
        "evidence",
        sqlx::query!(
            "UPDATE evidence SET note = $3, note_revision = note_revision + 1, note_updated_by = $5,
                                 note_updated_at = now()
             WHERE guild_id = $1 AND id = $2 AND note_revision = $4 AND removed_at IS NULL
               AND note IS DISTINCT FROM $3",
            i64::from(guild),
            evidence.get(),
            note,
            i32::try_from(revision).unwrap_or(i32::MAX),
            i64::from(by),
        )
        .execute(conn),
    )
    .await?;
    Ok(Edited::of(done.rows_affected()))
}

/// Shows an uploaded-files item to the member, or hides it.
pub async fn set_visibility(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    visible: bool,
) -> Result<Edited, DbError> {
    let done = obs::run(
        Update,
        "evidence",
        sqlx::query!(
            "UPDATE evidence SET visible_to_member = $3
             WHERE guild_id = $1 AND id = $2 AND kind = 'upload' AND removed_at IS NULL
               AND visible_to_member <> $3",
            i64::from(guild),
            evidence.get(),
            visible,
        )
        .execute(conn),
    )
    .await?;
    Ok(Edited::of(done.rows_affected()))
}

/// Removes an item: hidden from every view but the staff dashboard's
/// removed list; the row, its files, and their copies stay.
pub async fn remove(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
    by: AccountId,
) -> Result<Edited, DbError> {
    let done = obs::run(
        Update,
        "evidence",
        sqlx::query!(
            "UPDATE evidence SET removed_at = now(), removed_by = $3
             WHERE guild_id = $1 AND id = $2 AND removed_at IS NULL",
            i64::from(guild),
            evidence.get(),
            i64::from(by),
        )
        .execute(conn),
    )
    .await?;
    Ok(Edited::of(done.rows_affected()))
}
