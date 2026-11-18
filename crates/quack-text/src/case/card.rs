//! The staff card (§11.7, §12.4).

use quack_core::action::ActionState;
use quack_core::appeal::AppealState;
use quack_core::case::CaseSource;
use quack_core::ids::{ActionId, CaseId, MemberId};
use time::OffsetDateTime;

use super::{CONTEXT_CUT, ContextLine, context_value};
use crate::action::{self, ActionView, Detail, Undo};
use crate::actions::ButtonAction;
use crate::dm::{self, DmView};
use crate::fmt;
use crate::model::{Actor, Block, Button, ButtonStyle, Msg, Person, Text, TimeStyle, Tone};
use crate::word::{self, Outcome};

/// An action on the card with the controls `quack_core::action::controls`
/// allows right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardAction {
    /// The action.
    pub id: ActionId,
    /// Its facts.
    pub view: ActionView,
    /// Show "Retry {action}".
    pub can_retry: bool,
    /// Show "Dismiss {action}".
    pub can_dismiss: bool,
    /// Show "Remove timeout" / "Unban" (originals only).
    pub can_undo: bool,
}

/// How many evidence items a case has, by state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvidenceSummary {
    /// Still being saved.
    pub saving: u32,
    /// Saved.
    pub saved: u32,
    /// Couldn't be saved.
    pub failed: u32,
}

impl EvidenceSummary {
    /// Whether the case has any evidence.
    pub const fn any(self) -> bool {
        self.saving + self.saved + self.failed > 0
    }
}

/// Who voided a case, when, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voided {
    /// Who voided it (Quack never does on its own; an accepted appeal is
    /// the moderator who accepted it).
    pub by: Actor,
    /// When.
    pub at: OffsetDateTime,
    /// The reason the member can see.
    pub reason: String,
}

/// What Quack v4 recorded for an imported case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    /// The v4 action's name, as v4 stored it.
    pub v4_action: String,
    /// The v4 moderator, as v4 stored them.
    pub v4_moderator: String,
}

/// Everything the staff card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseCard {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The rule's name in the case's version.
    pub rule: String,
    /// The member.
    pub member: Person,
    /// The member's id, for the History button.
    pub member_id: MemberId,
    /// Where it came from.
    pub source: CaseSource,
    /// The 1-based level.
    pub level_index: u8,
    /// The level's starting case count.
    pub level_from: u32,
    /// The case count including this case.
    pub case_count: u32,
    /// The official reason.
    pub reason: String,
    /// The original action; `None` is a warning.
    pub action: Option<CardAction>,
    /// The undo of the original, if one exists.
    pub undo: Option<CardAction>,
    /// The case DM; `None` when the level sends none.
    pub dm: Option<DmView>,
    /// Evidence counts.
    pub evidence: EvidenceSummary,
    /// Answered context fields.
    pub context: Vec<ContextLine>,
    /// Set once voided.
    pub voided: Option<Voided>,
    /// The case this one replaces.
    pub replaces: Option<u64>,
    /// The case that replaces this one.
    pub replaced_by: Option<u64>,
    /// The case's appeal, if any.
    pub appeal: Option<AppealState>,
    /// Who added it (staff sources).
    pub moderator: Option<Person>,
    /// When it was added (or imported).
    pub created_at: OffsetDateTime,
    /// Set for imported cases.
    pub imported: Option<Imported>,
    /// The case on the dashboard.
    pub dashboard_url: String,
    /// The dashboard's failed actions page.
    pub failures_url: String,
}

pub(super) fn heading(number: u64, rule: &str, voided: bool) -> Block {
    let t = Text::fixed(format!("Case #{number} · ")).bold_user(rule);
    Block::Heading(if voided { t.s(" · Voided") } else { t })
}

pub(super) fn tone(action: Option<&ActionView>, voided: bool) -> Tone {
    if voided {
        return Tone::Neutral;
    }
    match action.map(action::outcome) {
        None | Some(Outcome::NotNeeded) => Tone::Neutral,
        Some(Outcome::Done) => Tone::Success,
        Some(Outcome::InProgress | Outcome::LeftInPlace | Outcome::Cancelled) => Tone::Warning,
        Some(Outcome::Failed | Outcome::CouldntConfirm) => Tone::Danger,
    }
}

fn evidence_line(e: EvidenceSummary) -> Option<Text> {
    if e.saving > 0 {
        Some(Text::fixed("Saving evidence…"))
    } else if e.failed > 0 {
        Some(Text::fixed(format!(
            "{} saved, {} couldn't be saved. Open View evidence to see why.",
            fmt::count(e.saved),
            fmt::count(e.failed)
        )))
    } else if e.saved > 0 {
        Some(Text::fixed(format!(
            "{} saved.",
            fmt::plural(u64::from(e.saved), "item", "items")
        )))
    } else {
        None
    }
}

fn appeal_line(state: AppealState, member: &Person) -> Text {
    match state {
        AppealState::Pending => Text::fixed("waiting for a decision."),
        AppealState::NeedsInformation => Text::fixed("waiting for ")
            .person(member)
            .s(" to add information."),
        AppealState::Accepted => Text::fixed("accepted."),
        AppealState::Rejected => Text::fixed("rejected."),
        AppealState::Closed => Text::fixed("closed."),
    }
}

pub(super) fn field(label: &str, value: Text) -> Block {
    Block::Field {
        label: label.to_owned(),
        value,
    }
}

fn needs_decision(a: &CardAction) -> bool {
    a.view.state == ActionState::Failed && a.view.dismissed.is_none()
}

fn buttons(c: &CaseCard) -> Vec<Button> {
    let mut b = vec![
        Button::new(
            "Edit context",
            ButtonStyle::Secondary,
            ButtonAction::EditContext(c.case),
        ),
        Button::new(
            "Add evidence",
            ButtonStyle::Secondary,
            ButtonAction::AddEvidence(c.case),
        ),
    ];
    if c.evidence.any() {
        b.push(Button::new(
            "View evidence",
            ButtonStyle::Secondary,
            ButtonAction::ViewEvidence {
                case: c.case,
                item: 1,
            },
        ));
    }
    b.push(Button::new(
        "History",
        ButtonStyle::Secondary,
        ButtonAction::History {
            member: c.member_id,
            page: 1,
        },
    ));
    if c.voided.is_none() {
        b.push(Button::new(
            "Void case",
            ButtonStyle::Danger,
            ButtonAction::VoidCase(c.case),
        ));
    }
    for a in c.action.iter().chain(c.undo.iter()) {
        if a.can_retry {
            b.push(Button::new(
                word::retry_label(a.view.kind),
                ButtonStyle::Primary,
                ButtonAction::RetryAction(a.id),
            ));
        }
        if a.can_dismiss {
            b.push(Button::new(
                word::dismiss_label(a.view.kind),
                ButtonStyle::Secondary,
                ButtonAction::DismissAction(a.id),
            ));
        }
    }
    let undo = c
        .action
        .as_ref()
        .filter(|a| a.can_undo)
        .and_then(|a| action::undo_from_original(a.view.kind).map(|u| (a.id, u)));
    if let Some((id, undo)) = undo {
        let label = match undo {
            Undo::RemoveTimeout => "Remove timeout",
            Undo::Unban => "Unban",
        };
        b.push(Button::new(
            label,
            ButtonStyle::Danger,
            ButtonAction::UndoAction(id),
        ));
    }
    if c.action.iter().chain(c.undo.iter()).any(needs_decision) {
        b.push(word::failed_actions_button(&c.failures_url));
    }
    b.push(word::dashboard_button(&c.dashboard_url));
    b
}

fn footer(c: &CaseCard) -> Option<Text> {
    let at = |t: Text| t.time(c.created_at, TimeStyle::Relative);
    match (c.source, &c.imported, &c.moderator) {
        (CaseSource::Imported, imported, _) => Some(
            at(Text::fixed("Imported "))
                .s(". Original moderator: ")
                .user(
                    imported
                        .as_ref()
                        .map_or("unknown", |i| i.v4_moderator.as_str()),
                ),
        ),
        (CaseSource::Honeypot, ..) => {
            Some(at(Text::fixed("Added by Quack ")).s(" after a post in the honeypot channel"))
        }
        (CaseSource::Discord | CaseSource::Dashboard, _, Some(m)) => {
            Some(at(Text::fixed("Added by ").person(m).s(" ")))
        }
        (CaseSource::Discord | CaseSource::Dashboard, _, None) => None,
    }
}

/// `case.card.*`: the staff card (also the private receipt). Ephemeral only.
pub fn card(c: &CaseCard) -> Msg {
    let imported = c.source == CaseSource::Imported;
    let mut msg = Msg::new(tone(c.action.as_ref().map(|a| &a.view), c.voided.is_some()))
        .block(heading(c.number, &c.rule, c.voided.is_some()));
    msg = msg.para(if imported {
        Text::new().person(&c.member).s(" · imported from Quack v4")
    } else {
        Text::new().person(&c.member).s(format!(
            " · {} · case count {}",
            fmt::level(c.level_index, c.level_from),
            c.case_count
        ))
    });
    msg = msg.block(Block::Quote(Text::new().user(&c.reason)));
    if !imported {
        let sentence = c.action.as_ref().map_or_else(action::none, |a| {
            action::sentence(&a.view, &c.member, Detail::Full)
        });
        msg = msg.block(field("Action", sentence));
        if let Some(u) = &c.undo {
            msg = msg.block(field(
                "Undo",
                action::sentence(&u.view, &c.member, Detail::Full),
            ));
        }
        msg = msg.block(field(
            "DM",
            dm::sentence(c.dm.as_ref(), &c.member, c.number),
        ));
    }
    if let Some(line) = evidence_line(c.evidence) {
        msg = msg.block(field("Evidence", line));
    }
    if !c.context.is_empty() {
        msg = msg.para(Text::new().bold("Case context"));
        for line in &c.context {
            msg = msg.block(Block::Field {
                label: fmt::one_line(&line.label),
                value: context_value(line, CONTEXT_CUT),
            });
        }
    }
    if let Some(v) = &c.voided {
        msg = msg
            .para(
                Text::fixed("Voided by ")
                    .actor(&v.by)
                    .s(" ")
                    .time(v.at, TimeStyle::Relative)
                    .s("."),
            )
            .block(Block::Quote(Text::new().user(&v.reason)));
    }
    if let Some(n) = c.replaces {
        msg = msg.para(Text::fixed(format!("Replaces case #{n}.")));
    }
    if let Some(n) = c.replaced_by {
        msg = msg.para(Text::fixed(format!("Replaced by case #{n}.")));
    }
    if let Some(state) = c.appeal {
        msg = msg.block(field("Appeal", appeal_line(state, &c.member)));
    }
    if c.action.iter().chain(c.undo.iter()).any(needs_decision) {
        msg = msg.para(Text::fixed(
            "A failed action needs a decision. Retry or dismiss it here, or see every failed action on the dashboard.",
        ));
    }
    if let (true, Some(i)) = (imported, &c.imported) {
        msg = msg.block(Block::Subtext(
            Text::fixed("The original Quack v4 action was ")
                .user(&i.v4_action)
                .s(". Quack took no action for this case."),
        ));
    }
    if let Some(f) = footer(c) {
        msg = msg.block(Block::Subtext(f));
    }
    msg.buttons(buttons(c))
}
