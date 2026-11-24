//! Custom ids (§11.5): `1:<code>[:<arg>]…`, at most 100 characters.
//!
//! `1` is the scheme version. UUIDs are written as 22 characters of
//! base64url (`u22`), snowflakes and numbers in decimal. A custom id carries
//! no authority: every handler authorizes the clicker again. Decoding never
//! panics; anything it can't read is [`ComponentId::Expired`] (a `1:` id)
//! or [`ComponentId::Unrecognized`].

mod actions;
mod legacy;
mod parse;
mod setup;

#[cfg(test)]
mod tests;

use quack_core::ids::{
    ActionId, AppealId, CaseId, ChannelId, EvidenceId, JobId, MemberId, MessageId, RuleId, TicketId,
};
use quack_text::actions::{DraftPage, SetupArea, SetupOp};

pub use actions::{decode_input_id, encode_input_id, encode_select_value};

/// Most characters Discord accepts in a custom id.
pub const MAX_LEN: usize = 100;

/// Every component Quack renders, one variant per row of §11.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomId {
    /// `cv`: show a case card.
    OpenCase(CaseId),
    /// `cvs`: the "Open a case" select (the value is the case).
    CasePicker,
    /// `ce`: open the edit-context modal.
    EditContext(CaseId),
    /// `cem`: the edit-context modal, opened at a context revision.
    EditContextModal {
        /// The case.
        case: CaseId,
        /// The context revision the modal showed.
        revision: u32,
    },
    /// `ea`: open the add-evidence modal.
    AddEvidence {
        /// The case.
        case: CaseId,
        /// `n`: answer in a new ephemeral message, never updating the
        /// message the button is on (a card or `case.receipt.posted`,
        /// which a receipt's sync owns, §11.7).
        new_message: bool,
    },
    /// `eam`: the add-evidence modal.
    AddEvidenceModal {
        /// The case.
        case: CaseId,
        /// `n`: answer in a new ephemeral message.
        new_message: bool,
    },
    /// `ev`: show one evidence item (1-based).
    ViewEvidence {
        /// The case.
        case: CaseId,
        /// Which item.
        item: u32,
        /// `n`: answer in a new ephemeral message.
        new_message: bool,
    },
    /// `evb`: back to an evidence item, found by id on click.
    BackToEvidence(EvidenceId),
    /// `evs`: the messages saved around an item's message.
    EvidenceSurrounding(EvidenceId),
    /// `evv`: show an uploaded-files item to the member (`1`) or hide it
    /// (`0`).
    EvidenceVisibility {
        /// The item.
        evidence: EvidenceId,
        /// Whether the member should see it.
        visible: bool,
    },
    /// `evn`: open the note modal (prefilled).
    EditEvidenceNote(EvidenceId),
    /// `evnm`: the note modal, opened at a note revision.
    EvidenceNoteModal {
        /// The item.
        evidence: EvidenceId,
        /// The note revision the modal showed.
        revision: u32,
    },
    /// `evr`: ask to confirm removing an item.
    RemoveEvidence(EvidenceId),
    /// `evrc`: remove after confirming.
    ConfirmRemoveEvidence(EvidenceId),
    /// `exs`: the "Add as evidence" case select (the value is the case).
    AddAsEvidence {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// `exn`: open the "Add as evidence" case number modal.
    AddAsEvidenceByNumber {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// `exm`: the "Add as evidence" case number modal.
    AddAsEvidenceModal {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// `vd`: open the void modal.
    VoidCase(CaseId),
    /// `vdm`: the void modal.
    VoidCaseModal(CaseId),
    /// `hi`: a member's history page.
    History {
        /// The member.
        member: MemberId,
        /// The 1-based page.
        page: u32,
    },
    /// `cl`: a `/case list` page.
    CaseList {
        /// The 1-based page.
        page: u32,
        /// The member filter.
        member: Option<MemberId>,
        /// The rule filter.
        rule: Option<RuleId>,
    },
    /// `ar`: retry a failed action.
    RetryAction(ActionId),
    /// `ad`: dismiss a failed action.
    DismissAction(ActionId),
    /// `au`: ask to confirm undoing an original action.
    UndoAction(ActionId),
    /// `auc`: undo after confirming.
    ConfirmUndo(ActionId),
    /// `rp`: the rule picker select of a draft (the value is the rule).
    RulePicker(CaseId),
    /// `rpp`: a rule picker page.
    RulePickerPage {
        /// The draft.
        draft: CaseId,
        /// The 1-based page.
        page: u32,
    },
    /// `cd`: a `/case add` form page modal.
    DraftPageModal(DraftPage),
    /// `cn`: open a draft page's modal.
    DraftContinue(DraftPage),
    /// `cr`: re-open a draft page with the values that had problems.
    DraftRetryPage(DraftPage),
    /// `cs`: apply a completed draft again.
    DraftSaveAgain(CaseId),
    /// `ap`: open the appeal modal (from a DM).
    Appeal(CaseId),
    /// `apm`: the appeal modal.
    AppealModal(CaseId),
    /// `aa`: open the accept modal.
    AcceptAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// `aam`: the accept modal.
    AcceptAppealModal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// `aj`: open the reject modal.
    RejectAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// `ajm`: the reject modal.
    RejectAppealModal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// `ao`: show the appeal card.
    OpenAppeal(AppealId),
    /// `al`: an `/appeals` page.
    AppealsPage(u32),
    /// `rl`: a `/rules` page.
    RulesPage(u32),
    /// `su`: a `/setup` button or select.
    Setup(SetupArea, SetupOp),
    /// `su:ap:invm`: the rejoin invite modal.
    SetupInviteModal,
    /// `su:hp:warnm`: the honeypot warning modal.
    SetupWarningModal,
    /// `to`: open a ticket.
    OpenTicket,
    /// `tc`: close a ticket.
    CloseTicket(TicketId),
    /// `tf`: fix a ticket's access.
    FixTicketAccess(TicketId),
    /// `op:st`: refresh the ops status board.
    OpsRefresh,
    /// `op:rq`: requeue a dead job.
    OpsRequeueJob(JobId),
    /// `op:dm`: dismiss a dead job.
    OpsDismissJob(JobId),
    /// `op:jp`: a `/ops jobs` page.
    OpsJobsPage(u32),
    /// `op:sv`: a `/ops metrics servers` page in an order.
    OpsServersPage(quack_text::ops::ServerSort, u32),
}

/// What a received custom id means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentId {
    /// A current (`1:`) id.
    Current(CustomId),
    /// An id from a message the Go version posted.
    Legacy(Legacy),
    /// A `1:` id that doesn't decode: answer `interaction.expired_button`.
    Expired,
    /// Anything else: answer `interaction.legacy_button`.
    Unrecognized,
}

/// A supported Go-era id (§11.5). The mapped record must belong to the
/// clicker's server; handlers check that like any other id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Legacy {
    /// Handled exactly as this current id.
    Mapped(CustomId),
    /// Show the case of this (mapped) action, view only.
    CaseOfAction(ActionId),
}

impl CustomId {
    /// Writes the id. Every variant fits in [`MAX_LEN`] characters.
    pub fn encode(&self) -> String {
        parse::encode(self)
    }
}

/// Reads a received custom id. Never panics.
pub fn decode(raw: &str) -> ComponentId {
    if raw.len() > MAX_LEN * 4 {
        // Too long to be anything Quack wrote; a `1:` id still gets the
        // expired-button answer (§11.5), like any other malformed one.
        return if raw.starts_with("1:") {
            ComponentId::Expired
        } else {
            ComponentId::Unrecognized
        };
    }
    match raw.strip_prefix("1:") {
        Some(rest) => parse::decode(rest).map_or(ComponentId::Expired, ComponentId::Current),
        None => legacy::decode(raw).map_or(ComponentId::Unrecognized, ComponentId::Legacy),
    }
}
