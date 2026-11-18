//! Shared words (§12.2): action and outcome names, page counters, common
//! buttons, and how a level is summarised.

use quack_core::action::ActionKind;
use quack_core::ids::CaseId;
use quack_core::rule::LevelAction;

use crate::actions::{ButtonAction, SelectAction, SelectValue};
use crate::fmt;
use crate::model::{Button, ButtonStyle, Select, SelectOption, SelectOptions, Text};

/// `word.action.*`: an action's name in a sentence ("timeout").
pub const fn action(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::Timeout => "timeout",
        ActionKind::Kick => "kick",
        ActionKind::Ban => "ban",
        ActionKind::RemoveTimeout => "remove timeout",
        ActionKind::Unban => "unban",
    }
}

/// An action as the subject or object of a sentence ("this ban", "the
/// failed remove timeout action"); "remove timeout" alone isn't a noun.
pub const fn action_noun(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::RemoveTimeout => "remove timeout action",
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban | ActionKind::Unban => {
            action(kind)
        }
    }
}

/// `case.button.retry`: "Retry ban"; a remove timeout is "Retry timeout
/// removal", since "Retry remove timeout" doesn't read.
pub fn retry_label(kind: ActionKind) -> String {
    format!("Retry {}", button_noun(kind))
}

/// `case.button.dismiss`: "Dismiss ban", "Dismiss timeout removal".
pub fn dismiss_label(kind: ActionKind) -> String {
    format!("Dismiss {}", button_noun(kind))
}

const fn button_noun(kind: ActionKind) -> &'static str {
    match kind {
        ActionKind::RemoveTimeout => "timeout removal",
        ActionKind::Timeout | ActionKind::Kick | ActionKind::Ban | ActionKind::Unban => {
            action(kind)
        }
    }
}

/// The outcome of an action or DM, as shown in lists and badges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Waiting or running.
    InProgress,
    /// It happened.
    Done,
    /// Discord said no, or Quack gave up.
    Failed,
    /// Quack doesn't know whether it happened.
    CouldntConfirm,
    /// Nothing had to happen.
    NotNeeded,
    /// An original action that won't run because its case was voided.
    Cancelled,
    /// An undo that left the timeout or ban in place.
    LeftInPlace,
}

/// `word.outcome.*`.
pub const fn outcome(o: Outcome) -> &'static str {
    match o {
        Outcome::InProgress => "In progress",
        Outcome::Done => "Done",
        Outcome::Failed => "Failed",
        Outcome::CouldntConfirm => "Couldn't confirm",
        Outcome::NotNeeded => "Not needed",
        Outcome::Cancelled => "Cancelled",
        Outcome::LeftInPlace => "Left in place",
    }
}

/// `word.yes` / `word.no`.
pub const fn yes_no(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

/// `word.voided`.
pub const VOIDED: &str = "Voided";
/// `word.imported`.
pub const IMPORTED: &str = "Imported";

/// `word.page`: "Page 2 of 5".
pub fn page(page: u32, pages: u32) -> String {
    format!("Page {page} of {pages}")
}

/// `button.previous` and `button.next` for a pager, built from the page
/// and the action that shows a given page. Nothing for a single page.
pub fn pager(page: u32, pages: u32, to: impl Fn(u32) -> ButtonAction) -> Vec<Button> {
    let mut buttons = Vec::new();
    if page > 1 {
        buttons.push(Button::new(
            "Previous",
            ButtonStyle::Secondary,
            to(page - 1),
        ));
    }
    if page < pages {
        buttons.push(Button::new("Next", ButtonStyle::Secondary, to(page + 1)));
    }
    buttons
}

/// `button.open_case`.
pub fn open_case_button(case: CaseId) -> Button {
    Button::new(
        "Open case",
        ButtonStyle::Secondary,
        ButtonAction::OpenCase(case),
    )
}

/// `button.open_in_dashboard`.
pub fn dashboard_button(url: impl Into<String>) -> Button {
    Button::link("Open in dashboard", url)
}

/// `button.failed_actions`: the dashboard's failed actions page.
pub fn failed_actions_button(url: impl Into<String>) -> Button {
    Button::link("Failed actions", url)
}

/// One row of an "Open a case" select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseOption {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// Its rule's name.
    pub rule: String,
}

/// `select.open_case`: a select over the cases of a list page; `None` when
/// the page has no cases.
pub fn open_case_select(cases: &[CaseOption]) -> Option<Select> {
    if cases.is_empty() {
        return None;
    }
    let options = cases
        .iter()
        .take(crate::budget::SELECT_OPTIONS)
        .map(|c| {
            SelectOption::new(
                &format!("#{} · {}", c.number, fmt::one_line(&c.rule)),
                None,
                SelectValue::Case(c.case),
            )
        })
        .collect();
    Some(Select {
        action: SelectAction::OpenCase,
        placeholder: "Open a case".to_owned(),
        options: SelectOptions::Strings(options),
    })
}

/// `render.trimmed`.
pub fn render_trimmed() -> Text {
    Text::fixed("Some details are only on the dashboard.")
}

/// `level.outcome.*`: what a level does ("timeout for 1 hour").
pub fn level_outcome(action: Option<LevelAction>) -> String {
    match action {
        None => "warning".to_owned(),
        Some(LevelAction::Timeout { duration_seconds }) => {
            format!("timeout for {}", fmt::duration(u64::from(duration_seconds)))
        }
        Some(LevelAction::Kick) => "kick".to_owned(),
        Some(LevelAction::Ban {
            delete_message_seconds: 0,
        }) => "ban".to_owned(),
        Some(LevelAction::Ban {
            delete_message_seconds,
        }) => format!(
            "ban and delete the last {} of messages",
            fmt::last(u64::from(delete_message_seconds))
        ),
    }
}

/// `level.summary.item`: "from case 3: timeout for 1 hour".
pub fn level_summary_item(from_case_count: u32, action: Option<LevelAction>) -> String {
    format!("from case {from_case_count}: {}", level_outcome(action))
}

/// Every level of a rule as `level.summary.item`s joined with " · " (the
/// `levels_text` of rule lists).
pub fn levels_summary(levels: &[(u32, Option<LevelAction>)]) -> String {
    levels
        .iter()
        .map(|(from, action)| level_summary_item(*from, *action))
        .collect::<Vec<_>>()
        .join(" · ")
}
