//! What a button, select, or modal does, as typed values.
//!
//! These carry ids, never authority: `quack-discord` encodes each one as a
//! custom id (§11.5), and every click is authorized again when it arrives.

use quack_core::ids::{
    ActionId, AppealId, CaseId, ChannelId, EvidenceId, JobId, MemberId, MessageId, RuleId, TicketId,
};
use quack_core::rule::FieldKey;

/// A `/setup` area (§11.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupArea {
    /// The home panel.
    Home,
    /// The log channel.
    Log,
    /// The appeal channel and rejoin invite.
    Appeals,
    /// The evidence channel.
    Evidence,
    /// Tickets.
    Tickets,
    /// The honeypot.
    Honeypot,
    /// The server activity log.
    ActivityLog,
}

/// A `/setup` control (§11.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOp {
    /// Show the area.
    Open,
    /// Choose the area's channel (`ch`).
    Channel,
    /// Create the area's channel or channels (`new`).
    Create,
    /// Clear the area's channel (`clr`).
    Clear,
    /// Set the rejoin invite (`inv`).
    SetInvite,
    /// Remove the rejoin invite (`clrinv`).
    ClearInvite,
    /// Choose the ticket entry channel (`entry`).
    TicketEntry,
    /// Choose the ticket staff channel (`queue`).
    TicketQueue,
    /// Choose the honeypot rule (`rule`).
    Rule,
    /// Clear the honeypot rule (`clrrule`).
    ClearRule,
    /// Edit the honeypot warning (`warn`).
    EditWarning,
    /// Turn the module on (`on`).
    TurnOn,
    /// Turn the module off (`off`).
    TurnOff,
}

/// The form a multi-page `/case add` is on (§11.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftPage {
    /// The draft (a `UUIDv7`, also used before the row exists).
    pub draft: CaseId,
    /// The rule being applied.
    pub rule: RuleId,
    /// The rule version the draft started with.
    pub version: u32,
    /// The 1-based form page.
    pub page: u8,
}

/// What a button does when clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    /// Show a case card (`cv`).
    OpenCase(CaseId),
    /// Open the edit-context modal (`ce`).
    EditContext(CaseId),
    /// Open the add-evidence modal (`ea`).
    AddEvidence {
        /// The case.
        case: CaseId,
        /// Answer in a new ephemeral message instead of updating the one
        /// the button is on: set on a card and on `case.receipt.posted`,
        /// whose message a receipt's sync owns (§11.7).
        new_message: bool,
    },
    /// Show one evidence item, 1-based among items that aren't removed
    /// (`ev`).
    ViewEvidence {
        /// The case.
        case: CaseId,
        /// Which item.
        item: u32,
        /// Answer in a new ephemeral message (see
        /// [`ButtonAction::AddEvidence`]).
        new_message: bool,
    },
    /// Back to an evidence item from its surrounding messages or its
    /// remove confirmation (`evb`): its position is found on click.
    BackToEvidence(EvidenceId),
    /// Show the messages saved around an item's message (`evs`).
    EvidenceSurrounding(EvidenceId),
    /// Show an uploaded-files item to the member, or hide it (`evv`).
    EvidenceVisibility {
        /// The item.
        evidence: EvidenceId,
        /// Whether the member should see it.
        visible: bool,
    },
    /// Open the note modal, prefilled (`evn`).
    EditEvidenceNote(EvidenceId),
    /// Ask to confirm removing an item (`evr`).
    RemoveEvidence(EvidenceId),
    /// Remove an item after confirming (`evrc`).
    ConfirmRemoveEvidence(EvidenceId),
    /// "Add as evidence": open the case number modal (`exn`).
    AddAsEvidenceByNumber {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// Open the void modal (`vd`).
    VoidCase(CaseId),
    /// Show a member's history page (`hi`).
    History {
        /// The member.
        member: MemberId,
        /// The 1-based page.
        page: u32,
    },
    /// Show a `/case list` page (`cl`).
    CaseListPage {
        /// The 1-based page.
        page: u32,
        /// The member filter.
        member: Option<MemberId>,
        /// The rule filter.
        rule: Option<RuleId>,
    },
    /// Retry a failed action (`ar`).
    RetryAction(ActionId),
    /// Dismiss a failed action (`ad`).
    DismissAction(ActionId),
    /// Ask to confirm undoing an original action (`au`).
    UndoAction(ActionId),
    /// Undo an original action after confirming (`auc`).
    ConfirmUndo(ActionId),
    /// Show a rule picker page (`rpp`).
    RulePickerPage {
        /// The draft.
        draft: CaseId,
        /// The 1-based page.
        page: u32,
    },
    /// Open a draft page's modal (`cn`).
    DraftContinue(DraftPage),
    /// Re-open a draft page with the values that had problems (`cr`).
    DraftRetryPage(DraftPage),
    /// Apply a completed draft again (`cs`).
    DraftSaveAgain(CaseId),
    /// Open the appeal modal from a DM (`ap`).
    Appeal(CaseId),
    /// Open the accept modal (`aa`).
    AcceptAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// Open the reject modal (`aj`).
    RejectAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// Show the appeal card (`ao`).
    OpenAppeal(AppealId),
    /// Show an `/appeals` page (`al`).
    AppealsPage(u32),
    /// Show a `/rules` page (`rl`).
    RulesPage(u32),
    /// A `/setup` control (`su`).
    Setup(SetupArea, SetupOp),
    /// Open a ticket (`to`).
    OpenTicket,
    /// Close a ticket (`tc`).
    CloseTicket(TicketId),
    /// Fix a ticket's access (`tf`).
    FixTicketAccess(TicketId),
    /// Refresh the ops status board (`op:st`).
    OpsRefresh,
    /// Requeue a dead job (`op:rq`).
    OpsRequeueJob(JobId),
    /// Dismiss a dead job (`op:dm`).
    OpsDismissJob(JobId),
    /// Show a `/ops jobs` page (`op:jp`).
    OpsJobsPage(u32),
    /// Show a `/ops metrics servers` page (1-based) in an order (`op:sv`).
    OpsServersPage(crate::ops::ServerSort, u32),
}

/// What choosing in a select does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectAction {
    /// Open the chosen case (`cvs`).
    OpenCase,
    /// Set a draft's rule (`rp`).
    PickRule(CaseId),
    /// "Add as evidence": add the message to the chosen case (`exs`).
    AddAsEvidence {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// A `/setup` select (`su`): a channel or the honeypot rule.
    Setup(SetupArea, SetupOp),
}

/// The value behind one select option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectValue {
    /// A case.
    Case(CaseId),
    /// A rule.
    Rule(RuleId),
    /// Yes, in a Yes / No input.
    Yes,
    /// No, in a Yes / No input.
    No,
}

/// What submitting a modal does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModalAction {
    /// Save a case's context (`cem`).
    EditContext {
        /// The case.
        case: CaseId,
        /// The context revision the modal was opened at.
        revision: u32,
    },
    /// Add evidence: links, files, and a note (`eam`).
    AddEvidence {
        /// The case.
        case: CaseId,
        /// Answer in a new ephemeral message: the modal opened from a card
        /// or `case.receipt.posted` (see [`ButtonAction::AddEvidence`]).
        new_message: bool,
    },
    /// Save an item's note (`evnm`).
    EvidenceNote {
        /// The item.
        evidence: EvidenceId,
        /// The note revision the modal was opened at.
        revision: u32,
    },
    /// "Add as evidence" by case number, with a note (`exm`).
    AddAsEvidence {
        /// The message's channel.
        channel: ChannelId,
        /// The message.
        message: MessageId,
    },
    /// Void a case (`vdm`).
    VoidCase(CaseId),
    /// Save a `/case add` form page (`cd`).
    DraftPage(DraftPage),
    /// Submit an appeal (`apm`).
    Appeal(CaseId),
    /// Accept an appeal (`aam`).
    AcceptAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// Reject an appeal (`ajm`).
    RejectAppeal {
        /// The appeal.
        appeal: AppealId,
        /// The version the clicker saw.
        version: u32,
    },
    /// Save the rejoin invite (`invm`).
    SetupInvite,
    /// Save the honeypot warning (`warnm`).
    SetupWarning,
}

/// Which value a modal input holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputId {
    /// A case context field.
    Field(FieldKey),
    /// A reason (void, appeal decision).
    Reason,
    /// Message links, one per line.
    Links,
    /// Uploaded files (a file upload field).
    Files,
    /// A staff note.
    Note,
    /// A case number.
    CaseNumber,
    /// The member's appeal statement.
    Statement,
    /// The rejoin invite link.
    Invite,
    /// The honeypot warning text.
    Warning,
}
