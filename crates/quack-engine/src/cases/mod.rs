//! Cases (§4): applying a rule, voiding, editing context, adding evidence,
//! and the staff and member read models.

mod action_list;
mod apply;
mod apply_tx;
mod apply_writes;
mod card;
mod edit;
mod evidence_add;
mod facts;
mod list;
mod records;
mod timeline;
mod view;
mod view_items;
mod view_member;
mod void;

use quack_core::DomainError;
use quack_core::case::{ContextInput, ContextProblem, LinkProblem, MessageLink};
use quack_core::evidence::RefusedFile;
use quack_core::ids::{
    ActionId, CaseId, ChannelId, DmId, EvidenceId, MemberId, MessageRowId, RuleId,
};
use quack_db::DbError;
use quack_db::cases::Case;
use time::OffsetDateTime;

use crate::opkey::KeyError;
use crate::port::InteractionToken;
use crate::principal::Principal;

pub use action_list::{ListedAction, action, actions};
pub use apply::apply_rule;
pub(crate) use card::{card, receipt};
pub use edit::{ContextEdited, edit_context};
pub use evidence_add::{AddEvidence, EvidenceAdded, add_evidence};
pub use list::{CasePreview, list, member_cases, member_summary, preview};
pub(crate) use records::CaseRecords;
pub use view::{
    AppealItem, CaseView, DmItem, TimelineItem, TimelineText, VoidItem, card as staff_card, get,
    get_by_number,
};
pub use view_items::{ActionItem, AttemptItem, EvidenceFileItem, EvidenceItem};
pub use view_member::{
    MemberAppeal, MemberAppealEvent, MemberCaseView, MemberContextLine, MemberEmbed,
    MemberEvidence, MemberEvidenceFile, member_case,
};
pub(crate) use void::{VoidBy, void_in_tx, voided_metrics};
pub use void::{VoidEffects, Voided, void_case, void_results, void_results_of};

/// Who applies a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    /// A moderator (Discord or dashboard).
    Staff(Principal),
    /// Quack itself (the honeypot module).
    Quack,
}

/// Where the case receipt goes (§11.7). The dashboard creates none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Receipt {
    /// The interaction's ephemeral original response, edited through its
    /// token until it expires.
    Private {
        /// The channel the command was used in.
        channel: ChannelId,
        /// The interaction token.
        token: InteractionToken,
        /// When the token stops being used (15 minutes minus 60 seconds).
        expires_at: OffsetDateTime,
    },
    /// A message Quack posts in the channel; the token is only used to
    /// tell the moderator privately that it was posted or refused.
    Public {
        /// Where to post it.
        channel: ChannelId,
        /// The moderator's interaction token and its expiry.
        token: Option<(InteractionToken, OffsetDateTime)>,
    },
}

/// The honeypot configuration a batch of hits was admitted under; the
/// case transaction checks it still holds (§4.2 step 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoneypotAdmission {
    /// `honeypot_settings.version` when the hits were admitted.
    pub settings_version: u64,
    /// The honeypot channel then.
    pub channel: ChannelId,
}

/// Everything one application of a rule needs (§4.2).
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyRule {
    /// The server.
    pub guild: quack_core::ids::GuildId,
    /// Who applies it.
    pub actor: Actor,
    /// The member the case is about.
    pub member: MemberId,
    /// The rule.
    pub rule: RuleId,
    /// The version the form was built from (a draft's, or the dashboard
    /// preview's): a different current version is `rule_changed`.
    pub pinned_version: Option<u32>,
    /// Submitted case context.
    pub context: Vec<(String, ContextInput)>,
    /// Explicit evidence links (dashboard).
    pub evidence_links: Vec<String>,
    /// Files uploaded with the case (the `/case add` file option): one
    /// uploaded-files item, refused up front unless Quack can copy them.
    pub evidence_files: Vec<crate::evidence::NewUpload>,
    /// The message an "Add case" message command was used on (or the
    /// honeypot post).
    pub source_message: Option<MessageLink>,
    /// The receipt destination; `None` on the dashboard and for honeypot
    /// cases.
    pub receipt: Option<Receipt>,
    /// The voided case this one replaces.
    pub replaces: Option<CaseId>,
    /// The operation key.
    pub op_key: crate::OpKey,
    /// The draft this application completes; the case transaction deletes
    /// it (§11.6).
    pub draft: Option<uuid::Uuid>,
    /// For honeypot cases: the configuration the hits were admitted under.
    pub honeypot: Option<HoneypotAdmission>,
}

/// What [`apply_rule`] did.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// A new case.
    Created(Box<CaseCreated>),
    /// The same request already created this case; render its current
    /// state for the actor.
    Replayed(CaseId),
}

impl Applied {
    /// The case either way.
    pub fn case_id(&self) -> CaseId {
        match self {
            Self::Created(created) => created.case.id,
            Self::Replayed(id) => *id,
        }
    }
}

/// The records a new case created.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseCreated {
    /// The case.
    pub case: Case,
    /// Its original action, when the level has one.
    pub action: Option<ActionId>,
    /// Its DM, when the level sends one.
    pub dm: Option<DmId>,
    /// Its evidence items (`saving`), the uploaded-files item last.
    pub evidence: Vec<EvidenceId>,
    /// Its receipt's tracked message.
    pub receipt: Option<MessageRowId>,
}

/// Why a rule could not be applied. `Refused` carries every refusal a
/// person can cause (capability, 2FA, member checks, rule state, evidence
/// limit, replacement); the rest are input problems or infrastructure.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// A refusal with a `quack-text` message.
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The case context has problems (every one is listed).
    #[error("context invalid")]
    ContextInvalid(Vec<ContextProblem>),
    /// Explicit evidence links have problems, by position.
    #[error("evidence links invalid")]
    LinksInvalid(Vec<(usize, LinkProblem)>),
    /// Files offered with the case can't be copied
    /// (`error.files_not_allowed`); nothing was saved.
    #[error("files not allowed")]
    FilesRefused(Vec<RefusedFile>),
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// The server reached the highest case number (2^53 − 1).
    #[error("case numbers exhausted")]
    CaseNumbersExhausted,
    /// The honeypot's settings changed after the hits were admitted.
    #[error("honeypot policy changed")]
    HoneypotPolicyChanged,
    /// Discord didn't answer the facts the checks need; nothing was saved.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// The commit's outcome is unknown; repeating the same operation key
    /// returns the case if it was saved.
    #[error("unconfirmed")]
    Unconfirmed,
    /// The transaction's 3-second deadline passed before it committed;
    /// nothing was saved (`error.internal`).
    #[error("deadline exceeded")]
    Deadline,
    /// The database failed; nothing was saved.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for ApplyError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for ApplyError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// Why a void, context edit, or evidence addition was refused.
#[derive(Debug, thiserror::Error)]
pub enum CaseError {
    /// A refusal with a `quack-text` message.
    #[error("refused: {0:?}")]
    Refused(DomainError),
    /// The new case context has problems.
    #[error("context invalid")]
    ContextInvalid(Vec<ContextProblem>),
    /// Evidence links have problems, by position.
    #[error("evidence links invalid")]
    LinksInvalid(Vec<(usize, LinkProblem)>),
    /// The case context changed since the editor opened it.
    #[error("context changed")]
    ContextChanged,
    /// The void reason is empty or longer than 1,000 characters.
    #[error("reason length")]
    ReasonLength,
    /// Adding evidence named no message link and no file
    /// (`error.no_links`).
    #[error("no evidence links")]
    NoLinks,
    /// Offered files can't be copied (`error.files_not_allowed`).
    #[error("files not allowed")]
    FilesRefused(Vec<RefusedFile>),
    /// The note is longer than 500 characters (`error.note_too_long`).
    #[error("note too long")]
    NoteLength,
    /// Discord didn't answer a check the operation needs; nothing changed.
    #[error("Discord unavailable")]
    DiscordUnavailable,
    /// The operation key was used for a different request.
    #[error("operation key reused")]
    OperationKeyReused,
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<DomainError> for CaseError {
    fn from(error: DomainError) -> Self {
        Self::Refused(error)
    }
}

impl From<KeyError> for CaseError {
    fn from(error: KeyError) -> Self {
        match error {
            KeyError::Reused => Self::OperationKeyReused,
            KeyError::Db(db) => Self::Db(db),
        }
    }
}

/// A case named by staff: by number (`/case view`, dashboard URLs) or id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseRef {
    /// The case number in the server.
    Number(u64),
    /// The case's id.
    Id(CaseId),
}

impl CaseRef {
    /// The case of `guild` this names.
    pub(crate) async fn find(
        self,
        conn: &mut quack_db::PgConnection,
        guild: quack_core::ids::GuildId,
    ) -> Result<Option<Case>, DbError> {
        match self {
            Self::Number(n) => quack_db::cases::get_by_number(conn, guild, n).await,
            Self::Id(id) => quack_db::cases::get(conn, guild, id).await,
        }
    }
}

/// Parses explicit evidence links (dashboard, add evidence): each must be
/// a message link into this server (§7.1).
pub(crate) fn parse_links(
    guild: quack_core::ids::GuildId,
    links: &[String],
) -> Result<Vec<MessageLink>, Vec<(usize, LinkProblem)>> {
    let mut parsed = Vec::new();
    let mut problems = Vec::new();
    for (i, link) in links.iter().enumerate() {
        match MessageLink::parse_in(link, guild) {
            Ok(link) => parsed.push(link),
            Err(problem) => problems.push((i, problem)),
        }
    }
    if problems.is_empty() {
        Ok(parsed)
    } else {
        Err(problems)
    }
}
