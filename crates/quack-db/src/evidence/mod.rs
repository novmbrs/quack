//! Evidence (§7): saved message snapshots and their files.

mod files;

use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
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

pub use files::{
    NewFile, Snapshot, fail_pending_files, file, files_for_case, mark_copied, mark_failed,
    mark_saved, pending_files, release_upload, reserve_upload, save_snapshot, set_file_state,
    start_copy,
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

/// One saved message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The item.
    pub id: EvidenceId,
    /// Its server.
    pub guild: GuildId,
    /// Its case.
    pub case: CaseId,
    /// The message's channel.
    pub channel: ChannelId,
    /// The message.
    pub message: MessageId,
    /// Who added it; `None` = Quack.
    pub added_by: Option<AccountId>,
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
    pub fn snapshot_stored(&self) -> bool {
        self.message_created_at.is_some()
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
    /// The original URL.
    pub original_url: String,
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
    /// Already `saving` or `saved`: nothing changed.
    Already(EvidenceId, EvidenceState),
}

/// Adds a message to a case's evidence, under the case lock (§7.1).
/// The caller enqueues `capture_evidence` for `Added` and `Recapture`.
/// [`DbError::NotFound`] when the case is not in `guild`; another server's
/// item is never read or changed.
pub async fn admit(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
    new_id: EvidenceId,
    channel: ChannelId,
    message: MessageId,
    added_by: Option<AccountId>,
) -> Result<Admission, DbError> {
    let row = obs::run(
        Insert,
        "evidence",
        sqlx::query!(
            r#"INSERT INTO evidence (id, guild_id, case_id, channel_id, message_id, added_by)
               SELECT $1, c.guild_id, c.id, $4, $5, $6 FROM cases c WHERE c.guild_id = $2 AND c.id = $3
               ON CONFLICT (case_id, message_id) DO UPDATE SET state = 'saving', failure = NULL
                 WHERE evidence.state = 'failed' AND evidence.guild_id = EXCLUDED.guild_id
               RETURNING id, (old.id IS NULL) AS "inserted!""#,
            new_id.get(),
            i64::from(guild),
            case.get(),
            i64::from(channel),
            i64::from(message),
            added_by.map(i64::from),
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    if let Some(r) = row {
        let id = EvidenceId::from_uuid(r.id);
        return Ok(if r.inserted {
            Admission::Added(id)
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

/// Evidence items of a case (the per-case cap is counted under its lock).
pub async fn count_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "evidence",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM evidence WHERE guild_id = $1 AND case_id = $2"#,
            i64::from(guild),
            case.get()
        )
        .fetch_one(conn),
    )
    .await?;
    count(n)
}

/// Saved files of a case (the 100-file cap).
pub async fn count_files_for_case(
    conn: &mut PgConnection,
    guild: GuildId,
    case: CaseId,
) -> Result<u32, DbError> {
    let n = obs::run(
        Select,
        "evidence_files",
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM evidence_files WHERE guild_id = $1 AND case_id = $2"#,
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
    channel_id: i64,
    message_id: i64,
    added_by: Option<i64>,
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
        Ok(Self {
            id: EvidenceId::from_uuid(r.id),
            guild: id(r.guild_id)?,
            case: CaseId::from_uuid(r.case_id),
            channel: id(r.channel_id)?,
            message: id(r.message_id)?,
            added_by: opt_id(r.added_by)?,
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
            source = r#"SELECT id, guild_id, case_id, channel_id, message_id, added_by, state, failure, author_id,
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
