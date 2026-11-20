//! Evidence (§7): saved message snapshots, uploaded files, notes,
//! visibility, and removal.

mod edits;
mod files;
mod uploads;

use quack_core::evidence::{EvidenceFailure, EvidenceKind, EvidenceState, FileCopyState};
use quack_core::ids::{
    AccountId, AttachmentId, CaseId, ChannelId, EvidenceId, FileId, GuildId, MessageId,
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::DbError;
use crate::obs::{self, Insert, Select};
use crate::pg::{Pg, count, id, opt_id};

pub use edits::{Edited, remove, set_note, set_visibility};
pub use files::{
    NewFile, Snapshot, fail_pending_files, file, files_for_case, mark_copied, mark_failed,
    mark_saved, pending_files, release_upload, reserve_upload, save_snapshot, set_file_state,
    start_copy,
};
pub use uploads::{
    NewUploadItem, StagedTotals, delete_staged, insert_upload, lock_staged_totals, staged_bytes,
};

/// The summary kept of one embed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedSummary {
    /// The embed type (`rich`, `image`, …).
    #[serde(rename = "type")]
    pub kind: String,
    /// Its title (at most 256 characters).
    pub title: Option<String>,
    /// Its description (at most 1,000 characters).
    pub description: Option<String>,
    /// Its URL.
    pub url: Option<String>,
}

/// One message saved around a linked message (§7.2 step 5): text only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurroundingMessage {
    /// The message.
    pub message_id: MessageId,
    /// Whether it came before the linked message (else after).
    pub before: bool,
    /// Its author.
    pub author_id: AccountId,
    /// Its text, cut to 500 characters; `None` when it had none.
    pub content: Option<String>,
    /// When it was sent.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// How many files it had (never copied).
    pub files: u32,
}

/// One evidence item: a saved message, or uploaded files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The item.
    pub id: EvidenceId,
    /// Its server.
    pub guild: GuildId,
    /// Its case.
    pub case: CaseId,
    /// A saved message or uploaded files.
    pub kind: EvidenceKind,
    /// The message's channel (`None` for uploaded files).
    pub channel: Option<ChannelId>,
    /// The message (`None` for uploaded files).
    pub message: Option<MessageId>,
    /// Who first added it; `None` = Quack.
    pub added_by: Option<AccountId>,
    /// Whose access the capture checks: the staff member whose request
    /// last started it (adding it, or adding it again after it failed);
    /// `None` = Quack. Use [`Evidence::capture_actor`].
    pub capture_by: Option<AccountId>,
    /// Whether the case's member sees it (always for a saved message).
    pub visible_to_member: bool,
    /// The staff note (at most 500 characters).
    pub note: Option<String>,
    /// Bumped by every note change; an edit names the revision it saw.
    pub note_revision: u32,
    /// Who last changed the note, and when.
    pub note_updated: Option<(OffsetDateTime, Option<AccountId>)>,
    /// When and by whom it was removed (hidden; the row stays).
    pub removed: Option<(OffsetDateTime, AccountId)>,
    /// Messages around a staff-added message; `None` when none were saved.
    pub surrounding: Option<Vec<SurroundingMessage>>,
    /// Its state.
    pub state: EvidenceState,
    /// Why it failed.
    pub failure: Option<EvidenceFailure>,
    /// The message's author, once saved.
    pub author: Option<AccountId>,
    /// The message's text (first 4,000 characters), once saved.
    pub content: Option<String>,
    /// When the message was sent; set exactly when the snapshot is stored.
    pub message_created_at: Option<OffsetDateTime>,
    /// When the message was last edited.
    pub message_edited_at: Option<OffsetDateTime>,
    /// Up to 10 embed summaries.
    pub embeds: Vec<EmbedSummary>,
    /// Files over the per-message or per-case cap, only counted.
    pub files_not_saved: u32,
    /// The copies were skipped because the evidence channel wasn't staff-only.
    pub copy_skipped_not_staff_only: bool,
    /// When it was added.
    pub created_at: OffsetDateTime,
    /// When it reached `saved`.
    pub saved_at: Option<OffsetDateTime>,
}

impl Evidence {
    /// Whether the snapshot itself was stored (files may still be pending).
    /// Uploaded files have no snapshot: always true.
    pub fn snapshot_stored(&self) -> bool {
        self.kind == EvidenceKind::Upload || self.message_created_at.is_some()
    }

    /// Whether staff removed it.
    pub const fn is_removed(&self) -> bool {
        self.removed.is_some()
    }

    /// The staff member whose access the capture checks (§7.2 step 3):
    /// [`Evidence::capture_by`], else (rows from before it existed) the
    /// adder; `None` = Quack.
    pub fn capture_actor(&self) -> Option<AccountId> {
        self.capture_by.or(self.added_by)
    }

    /// The saved message's location, for a message item.
    pub fn source(&self) -> Option<(ChannelId, MessageId)> {
        self.channel.zip(self.message)
    }
}

/// One file of a saved message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceFile {
    /// The file.
    pub id: FileId,
    /// Its evidence item.
    pub evidence: EvidenceId,
    /// Its case.
    pub case: CaseId,
    /// Its position in the message (1–10).
    pub position: u8,
    /// The original's Discord attachment ID.
    pub attachment_id: Option<AttachmentId>,
    /// The file name.
    pub filename: String,
    /// The content type, if Discord gave one.
    pub content_type: Option<String>,
    /// Its size.
    pub size_bytes: u64,
    /// The original URL: a message attachment's or an interaction
    /// attachment's CDN URL; `None` for a dashboard upload (its bytes are
    /// staged until copied).
    pub original_url: Option<String>,
    /// Its copy's state.
    pub copy_state: FileCopyState,
    /// When the current upload started (its nonce window).
    pub copy_started_at: Option<OffsetDateTime>,
    /// Where the copy is: channel, message, attachment.
    pub copy: Option<(ChannelId, MessageId, AttachmentId)>,
}

/// What [`admit`] did with a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// A new item, `saving`.
    Added(EvidenceId),
    /// An item whose capture had failed, back to `saving`.
    Recapture(EvidenceId),
    /// A removed item, shown again; `recapture` when its capture had
    /// failed and it is `saving` again.
    Restored {
        /// The item.
        id: EvidenceId,
        /// It is `saving` again and needs a capture job.
        recapture: bool,
    },
    /// Already `saving` or `saved` and not removed: nothing changed.
    Already(EvidenceId, EvidenceState),
}

impl Admission {
    /// The item, whatever happened.
    pub const fn id(self) -> EvidenceId {
        match self {
            Self::Added(id)
            | Self::Recapture(id)
            | Self::Restored { id, .. }
            | Self::Already(id, _) => id,
        }
    }

    /// Whether the caller must enqueue `capture_evidence` for it.
    pub const fn needs_capture(self) -> bool {
        matches!(
            self,
            Self::Added(_)
                | Self::Recapture(_)
                | Self::Restored {
                    recapture: true,
                    ..
                }
        )
    }
}

/// Adds a message to a case's evidence, under the case lock (§7.1). A
/// removed item comes back (and is captured again if its capture had
/// failed); a failed one is captured again. `added_by` is this request's
/// actor: a new item records it as its adder, and any item captured (again)
/// is captured under its access (`capture_by`), while an existing item
/// keeps its original adder. The caller checks that actor's access before
/// restoring or capturing an existing item again. `note` goes on a new
/// item, and on an existing one only when it has none. The caller enqueues
/// `capture_evidence` when [`Admission::needs_capture`].
/// [`DbError::NotFound`] when the case is not in `guild`; another server's
/// item is never read or changed.
pub async fn admit(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    new_id: EvidenceId,
    (channel, message): (ChannelId, MessageId),
    added_by: Option<AccountId>,
    note: Option<&str>,
) -> Result<Admission, DbError> {
    let row = obs::run(
        Insert,
        "evidence",
        sqlx::query!(
            r#"INSERT INTO evidence (id, guild_id, case_id, channel_id, message_id, added_by, capture_by,
                                     note, note_updated_by, note_updated_at)
               SELECT $1, c.guild_id, c.id, $4, $5, $6::bigint, $6::bigint, $7,
                      CASE WHEN $7::text IS NULL THEN NULL ELSE $6::bigint END,
                      CASE WHEN $7::text IS NULL THEN NULL ELSE now() END
               FROM cases c WHERE c.guild_id = $2 AND c.id = $3
               ON CONFLICT (case_id, message_id) DO UPDATE SET
                 state = CASE WHEN evidence.state = 'failed' THEN 'saving' ELSE evidence.state END,
                 failure = CASE WHEN evidence.state = 'failed' THEN NULL ELSE evidence.failure END,
                 capture_by = CASE WHEN evidence.state = 'failed' THEN EXCLUDED.capture_by
                                   ELSE evidence.capture_by END,
                 removed_at = NULL,
                 removed_by = NULL,
                 note = COALESCE(evidence.note, EXCLUDED.note),
                 note_revision = evidence.note_revision
                   + CASE WHEN evidence.note IS NULL AND EXCLUDED.note IS NOT NULL THEN 1 ELSE 0 END,
                 note_updated_by = CASE WHEN evidence.note IS NULL AND EXCLUDED.note IS NOT NULL
                                        THEN EXCLUDED.note_updated_by ELSE evidence.note_updated_by END,
                 note_updated_at = CASE WHEN evidence.note IS NULL AND EXCLUDED.note IS NOT NULL
                                        THEN EXCLUDED.note_updated_at ELSE evidence.note_updated_at END
                 WHERE (evidence.state = 'failed' OR evidence.removed_at IS NOT NULL)
                   AND evidence.guild_id = EXCLUDED.guild_id
               RETURNING id, (old.id IS NULL) AS "inserted!",
                         COALESCE(old.state = 'failed', false) AS "recapture!",
                         COALESCE(old.removed_at IS NOT NULL, false) AS "restored!""#,
            new_id.get(),
            i64::from(guild),
            case.get(),
            i64::from(channel),
            i64::from(message),
            added_by.map(i64::from),
            note,
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    if let Some(r) = row {
        let id = EvidenceId::from_uuid(r.id);
        return Ok(if r.inserted {
            Admission::Added(id)
        } else if r.restored {
            Admission::Restored {
                id,
                recapture: r.recapture,
            }
        } else {
            Admission::Recapture(id)
        });
    }
    let existing = obs::run(
        Select,
        "evidence",
        sqlx::query!(
            "SELECT id, state FROM evidence WHERE guild_id = $1 AND case_id = $2 AND message_id = $3",
            i64::from(guild),
            case.get(),
            i64::from(message)
        )
        .fetch_optional(conn),
    )
    .await?
    .ok_or(DbError::NotFound)?;
    Ok(Admission::Already(
        EvidenceId::from_uuid(existing.id),
        existing.state.0,
    ))
}

/// Which of these messages may never be evidence (§7.1): any message in
/// the server's evidence channel, and Quack's own copy posts wherever they
/// are now (a former evidence channel's copies stay copies). Returns those
/// messages.
pub async fn evidence_copies(
    conn: &mut PgConnection,
    guild: GuildId,
    messages: &[(ChannelId, MessageId)],
) -> Result<Vec<MessageId>, DbError> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let channels: Vec<i64> = messages.iter().map(|(c, _)| i64::from(*c)).collect();
    let ids: Vec<i64> = messages.iter().map(|(_, m)| i64::from(*m)).collect();
    let rows = obs::run(
        Select,
        "evidence_files",
        sqlx::query_scalar!(
            r#"SELECT l.m AS "message!" FROM unnest($2::bigint[], $3::bigint[]) AS l(c, m)
               WHERE l.c = (SELECT s.evidence_channel_id FROM guild_settings s WHERE s.guild_id = $1)
                  OR EXISTS (SELECT 1 FROM evidence_files f
                             WHERE f.guild_id = $1 AND f.copy_message_id = l.m AND f.copy_channel_id = l.c)"#,
            i64::from(guild),
            &channels,
            &ids,
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(id).collect()
}

/// Evidence items of a case that aren't removed (the 50-item cap, counted
/// under the case lock).
pub async fn count_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "evidence",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM evidence
               WHERE guild_id = $1 AND case_id = $2 AND removed_at IS NULL"#,
            i64::from(guild),
            case.get()
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Files of a case's items that aren't removed (the 100-file cap).
pub async fn count_files_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "evidence_files",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM evidence_files f JOIN evidence e ON e.id = f.evidence_id
               WHERE f.guild_id = $1 AND f.case_id = $2 AND e.removed_at IS NULL"#,
            i64::from(guild),
            case.get()
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

struct EvidenceRow {
    id: uuid::Uuid,
    guild_id: i64,
    case_id: uuid::Uuid,
    kind: Pg<EvidenceKind>,
    channel_id: Option<i64>,
    message_id: Option<i64>,
    added_by: Option<i64>,
    capture_by: Option<i64>,
    visible_to_member: bool,
    note: Option<String>,
    note_revision: i32,
    note_updated_by: Option<i64>,
    note_updated_at: Option<OffsetDateTime>,
    removed_at: Option<OffsetDateTime>,
    removed_by: Option<i64>,
    surrounding: Option<Json<Vec<SurroundingMessage>>>,
    state: Pg<EvidenceState>,
    failure: Option<Pg<EvidenceFailure>>,
    author_id: Option<i64>,
    content: Option<String>,
    message_created_at: Option<OffsetDateTime>,
    message_edited_at: Option<OffsetDateTime>,
    embeds: Json<Vec<EmbedSummary>>,
    files_not_saved: i32,
    copy_skipped_not_staff_only: bool,
    created_at: OffsetDateTime,
    saved_at: Option<OffsetDateTime>,
}

impl TryFrom<EvidenceRow> for Evidence {
    type Error = DbError;

    fn try_from(r: EvidenceRow) -> Result<Self, DbError> {
        let removed = match (r.removed_at, opt_id(r.removed_by)?) {
            (Some(at), Some(by)) => Some((at, by)),
            _ => None,
        };
        Ok(Self {
            id: EvidenceId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            case: CaseId::from_uuid(r.case_id),
            kind: r.kind.0,
            channel: opt_id(r.channel_id)?,
            message: opt_id(r.message_id)?,
            added_by: opt_id(r.added_by)?,
            capture_by: opt_id(r.capture_by)?,
            visible_to_member: r.visible_to_member,
            note: r.note,
            note_revision: count(r.note_revision)?,
            note_updated: match r.note_updated_at {
                Some(at) => Some((at, opt_id(r.note_updated_by)?)),
                None => None,
            },
            removed,
            surrounding: r.surrounding.map(|s| s.0),
            state: r.state.0,
            failure: r.failure.map(|f| f.0),
            author: opt_id(r.author_id)?,
            content: r.content,
            message_created_at: r.message_created_at,
            message_edited_at: r.message_edited_at,
            embeds: r.embeds.0,
            files_not_saved: count(r.files_not_saved)?,
            copy_skipped_not_staff_only: r.copy_skipped_not_staff_only,
            created_at: r.created_at,
            saved_at: r.saved_at,
        })
    }
}

macro_rules! query_evidence {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        ::sqlx::sqlx_macros::expand_query!(
            record = EvidenceRow,
            source = r#"SELECT id, guild_id, case_id, kind, channel_id, message_id, added_by, capture_by,
                               visible_to_member,
                               note, note_revision, note_updated_by, note_updated_at, removed_at, removed_by,
                               surrounding AS "surrounding: Json<Vec<SurroundingMessage>>",
                               state, failure, author_id,
                               content, message_created_at, message_edited_at,
                               embeds AS "embeds: Json<Vec<EmbedSummary>>", files_not_saved,
                               copy_skipped_not_staff_only, created_at, saved_at
                        FROM evidence "# + $tail,
            args = [$($arg),*]
        )
    };
}

/// One evidence item of this server.
pub async fn get(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
) -> Result<Option<Evidence>, DbError> {
    let row = obs::run(
        Select,
        "evidence",
        query_evidence!(
            "WHERE guild_id = $1 AND id = $2",
            i64::from(guild),
            evidence.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Evidence::try_from).transpose()
}

/// Locks an evidence item `FOR UPDATE` (after its case).
pub async fn lock(
    conn: &mut PgConnection,
    guild: GuildId,
    evidence: EvidenceId,
) -> Result<Option<Evidence>, DbError> {
    let row = obs::run(
        Select,
        "evidence",
        query_evidence!(
            "WHERE guild_id = $1 AND id = $2 FOR UPDATE",
            i64::from(guild),
            evidence.get()
        )
        .fetch_optional(conn),
    )
    .await?;
    row.map(Evidence::try_from).transpose()
}

/// A case's evidence, in the order it was added.
pub async fn for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<Vec<Evidence>, DbError> {
    let rows = obs::run(
        Select,
        "evidence",
        query_evidence!(
            "WHERE guild_id = $1 AND case_id = $2 ORDER BY created_at, id",
            i64::from(guild),
            case.get()
        )
        .fetch_all(conn),
    )
    .await?;
    rows.into_iter().map(Evidence::try_from).collect()
}
