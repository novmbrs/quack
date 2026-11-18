//! Adding a case (§11.6, §12.5): the rule picker, the case form, draft
//! replies, and case context problems.

use quack_core::case::{ContextProblem, ContextProblemCode, ContextValue};
use quack_core::ids::{CaseId, RuleId};
use quack_core::limits;
use quack_core::rule::{FieldKey, FieldKind};

use crate::actions::{ButtonAction, DraftPage, InputId, ModalAction, SelectAction, SelectValue};
use crate::budget;
use crate::fmt;
use crate::model::{
    Button, ButtonStyle, Input, InputKind, InputStyle, Modal, Msg, Person, Select, SelectOption,
    SelectOptions, Text, Tone,
};
use crate::word;

/// One active rule in the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerRule {
    /// The rule.
    pub id: RuleId,
    /// Its name.
    pub name: String,
    /// Its staff-facing description.
    pub description: String,
}

/// `case.picker.prompt` with the rule select (25 per page) and a pager.
pub fn picker(member: &Person, draft: CaseId, rules: &[PickerRule], page: u32, pages: u32) -> Msg {
    let options = rules
        .iter()
        .take(budget::SELECT_OPTIONS)
        .map(|r| {
            SelectOption::new(
                &fmt::one_line(&r.name),
                Some(&fmt::one_line(&r.description)),
                SelectValue::Rule(r.id),
            )
        })
        .collect();
    let mut msg = Msg::text(
        Tone::Neutral,
        Text::fixed("Choose a rule for the case about ")
            .person(member)
            .s("."),
    );
    if !rules.is_empty() {
        msg = msg.select(Select {
            action: SelectAction::PickRule(draft),
            placeholder: "Choose a rule".to_owned(),
            options: SelectOptions::Strings(options),
        });
    }
    msg.buttons(word::pager(page, pages, |page| {
        ButtonAction::RulePickerPage { draft, page }
    }))
}

/// `case.picker.no_rules`.
pub fn picker_no_rules() -> Text {
    Text::fixed(
        "This server has no active rules yet. Ask a rules manager to create one on the dashboard.",
    )
}

/// `case.picker.fill_form` with **Fill in the form**.
pub fn picker_fill_form(rule: &str, member: &Person, first: DraftPage) -> Msg {
    Msg::text(
        Tone::Neutral,
        Text::fixed("The rule ")
            .bold_user(rule)
            .s(" needs some answers. Fill in the form to finish the case about ")
            .person(member)
            .s("."),
    )
    .buttons(vec![Button::new(
        "Fill in the form",
        ButtonStyle::Primary,
        ButtonAction::DraftContinue(first),
    )])
}

/// One field of a form page, with the value to prefill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormField {
    /// The field's key.
    pub key: FieldKey,
    /// Its label (the question).
    pub label: String,
    /// What it takes.
    pub kind: FieldKind,
    /// Whether it must be answered.
    pub required: bool,
    /// What to prefill (Try again, Edit context).
    pub value: Option<ContextValue>,
}

/// One modal input for a context field. Labels over 45 characters are cut,
/// and the whole question goes in the description.
pub(crate) fn field_input(f: &FormField) -> Input {
    let label = fmt::one_line(&f.label);
    let description = (label.chars().count() > budget::INPUT_LABEL)
        .then(|| fmt::cut(&label, budget::INPUT_DESCRIPTION));
    let text = |style, placeholder: Option<&str>, max: usize| InputKind::Text {
        style,
        placeholder: placeholder.map(str::to_owned),
        min_length: u16::from(f.required),
        max_length: u16::try_from(max.min(budget::INPUT_MAX_LENGTH)).unwrap_or(4_000),
        value: match &f.value {
            Some(ContextValue::Text(s)) => Some(fmt::cut(s, max.min(budget::INPUT_MAX_LENGTH))),
            Some(ContextValue::Number(n)) => Some(n.to_string()),
            Some(ContextValue::Boolean(_)) | None => None,
        },
    };
    let kind = match f.kind {
        FieldKind::ShortText => text(InputStyle::Short, None, limits::CONTEXT_SHORT_TEXT.max),
        FieldKind::LongText => text(InputStyle::Paragraph, None, limits::CONTEXT_LONG_TEXT.max),
        FieldKind::Number => text(InputStyle::Short, Some("A number, like 3 or 2.5"), 40),
        FieldKind::MessageLink => text(
            InputStyle::Short,
            Some("https://discord.com/channels/…"),
            limits::CONTEXT_MESSAGE_LINK_MAX_CHARS,
        ),
        FieldKind::Boolean => InputKind::YesNo {
            placeholder: "Choose Yes or No".to_owned(),
            value: match f.value {
                Some(ContextValue::Boolean(b)) => Some(b),
                _ => None,
            },
        },
    };
    Input {
        id: InputId::Field(f.key.clone()),
        label: fmt::cut(&label, budget::INPUT_LABEL),
        description,
        required: f.required,
        kind,
    }
}

/// `case.form.title`: "{rule} ({page}/{pages})", the rule name cut so the
/// whole title fits 45 characters. A one-page form has no counter.
pub fn form_title(rule: &str, page: u8, pages: u8) -> String {
    let rule = fmt::one_line(rule);
    if pages <= 1 {
        return fmt::cut(&rule, budget::MODAL_TITLE);
    }
    let counter = format!(" ({page}/{pages})");
    let room = budget::MODAL_TITLE - counter.chars().count();
    format!("{}{counter}", fmt::cut(&rule, room))
}

/// The `/case add` form page modal (§11.6), at most 5 fields.
pub fn form_modal(rule: &str, at: DraftPage, pages: u8, fields: &[FormField]) -> Modal {
    Modal {
        action: ModalAction::DraftPage(at),
        title: form_title(rule, at.page, pages),
        inputs: fields
            .iter()
            .take(budget::MODAL_INPUTS)
            .map(field_input)
            .collect(),
    }
}

/// `case.draft.page_saved` with **Continue**.
pub fn draft_page_saved(saved_page: u8, pages: u8, next: DraftPage) -> Msg {
    Msg::text(
        Tone::Neutral,
        Text::fixed(format!(
            "Page {saved_page} of {pages} is saved in your draft. Continue to finish the case."
        )),
    )
    .buttons(vec![Button::new(
        "Continue",
        ButtonStyle::Primary,
        ButtonAction::DraftContinue(next),
    )])
}

/// `case.draft.page_invalid` with **Try again**: one problem per line.
pub fn draft_page_invalid(problems: &[Text], retry: DraftPage) -> Msg {
    let mut t = Text::fixed("Some answers need fixing, so this page isn't saved yet:");
    for p in problems {
        t = t.s("\n").then(p.clone());
    }
    Msg::text(Tone::Warning, t.s("\nUse Try again to correct them.")).buttons(vec![Button::new(
        "Try again",
        ButtonStyle::Primary,
        ButtonAction::DraftRetryPage(retry),
    )])
}

/// Where a double-clicked form page goes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlreadySaved {
    /// The draft is on this page now.
    Continue(DraftPage),
    /// The draft was already used for this case.
    Case(CaseId),
}

/// `case.draft.page_already_saved`, with the draft's current page or the
/// case it became.
pub fn draft_page_already_saved(next: AlreadySaved) -> Msg {
    let first = Text::fixed("This form page was already saved.");
    match next {
        AlreadySaved::Continue(page) => {
            Msg::text(Tone::Neutral, first.s(" Continue with the next page.")).buttons(vec![
                Button::new(
                    "Continue",
                    ButtonStyle::Primary,
                    ButtonAction::DraftContinue(page),
                ),
            ])
        }
        AlreadySaved::Case(case) => {
            Msg::text(Tone::Neutral, first).buttons(vec![word::open_case_button(case)])
        }
    }
}

/// `case.draft.expired`.
pub fn draft_expired() -> Text {
    Text::fixed("This form expired after 15 minutes. Run ")
        .command(crate::model::Command::CaseAdd)
        .s(" again.")
}

/// `case.draft.rule_changed`.
pub fn draft_rule_changed(rule: &str) -> Text {
    Text::fixed("The rule ")
        .bold_user(rule)
        .s(" changed while you were filling in the form. Run ")
        .command(crate::model::Command::CaseAdd)
        .s(" again to use the new version.")
}

/// An error from applying a completed draft, with **Try again** (`cs`).
pub fn draft_save_failed(error: Text, draft: CaseId) -> Msg {
    Msg::text(Tone::Danger, error).buttons(vec![Button::new(
        "Try again",
        ButtonStyle::Primary,
        ButtonAction::DraftSaveAgain(draft),
    )])
}

/// `context.*`: one problem with an answer. `label` is the field's label
/// (absent for keys that are not fields of the rule).
pub fn context_problem(
    problem: &ContextProblem,
    label: Option<&str>,
    kind: Option<FieldKind>,
) -> Text {
    let key = || Text::new().user(fmt::one_line(&problem.key));
    let lead = || {
        Text::new()
            .user(fmt::one_line(label.unwrap_or(&problem.key)))
            .s(": ")
    };
    match problem.code {
        ContextProblemCode::UnknownField => key().s(" isn't a field of this rule."),
        ContextProblemCode::DuplicateField => key().s(" is answered more than once."),
        ContextProblemCode::Required => lead().s("an answer is required."),
        ContextProblemCode::TooLong => {
            let max = match kind {
                Some(FieldKind::ShortText) => limits::CONTEXT_SHORT_TEXT.max,
                Some(FieldKind::MessageLink) => limits::CONTEXT_MESSAGE_LINK_MAX_CHARS,
                _ => limits::CONTEXT_LONG_TEXT.max,
            };
            lead().s(format!("use at most {} characters.", fmt::count(max as u64)))
        }
        ContextProblemCode::NotText => lead().s("enter text."),
        ContextProblemCode::NotYesNo => lead().s("choose Yes or No."),
        ContextProblemCode::NotANumber => lead().s("enter a number, like 3 or 2.5."),
        ContextProblemCode::NumberOutOfRange => lead().s(
            "enter a number between −1,000,000,000,000,000 and 1,000,000,000,000,000 with at most 6 decimal places.",
        ),
        ContextProblemCode::NotAMessageLink => lead().s(
            "paste a Discord message link. Right-click the message and choose Copy Message Link.",
        ),
        ContextProblemCode::OtherServer => {
            lead().s("that message is in another server. Use a message from this server.")
        }
    }
}
