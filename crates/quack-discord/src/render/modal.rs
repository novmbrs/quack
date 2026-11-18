//! The neutral `Modal` → a modal callback: one Label per input, wrapping a
//! Text Input or a Yes / No string select (§11.11).

use quack_text::model::{Input, InputKind, InputStyle, Modal};
use quack_text::{budget, word};
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::{
    Label, SelectMenu, SelectMenuOption, SelectMenuType, TextInput, TextInputStyle,
};

use crate::custom_id::{CustomId, encode_input_id};

/// A modal ready for an interaction callback (type 9).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedModal {
    /// The modal's custom id.
    pub custom_id: String,
    /// Its title (at most 45 characters).
    pub title: String,
    /// One Label per input.
    pub components: Vec<Component>,
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The `label` field is deprecated in favour of the Label component, which
/// Quack uses; the struct still has it.
#[allow(deprecated, reason = "TextInput::label is unused but still a field")]
fn input(id: String, kind: &InputKind, required: bool) -> Component {
    let InputKind::Text {
        style,
        placeholder,
        min_length,
        max_length,
        value,
    } = kind
    else {
        let (placeholder, value) = match kind {
            InputKind::YesNo { placeholder, value } => (placeholder.as_str(), *value),
            InputKind::Text { .. } => ("", None),
        };
        return yes_no(id, placeholder, value, required);
    };
    let max = (*max_length).clamp(1, u16::try_from(budget::INPUT_MAX_LENGTH).unwrap_or(4000));
    Component::TextInput(TextInput {
        custom_id: id,
        id: None,
        label: None,
        max_length: Some(max),
        min_length: (*min_length > 0).then_some((*min_length).min(max)),
        placeholder: placeholder
            .as_deref()
            .map(|p| cut(p, budget::INPUT_PLACEHOLDER)),
        required: Some(required),
        style: match style {
            InputStyle::Short => TextInputStyle::Short,
            InputStyle::Paragraph => TextInputStyle::Paragraph,
        },
        value: value
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(|v| cut(v, usize::from(max))),
    })
}

fn yes_no(id: String, placeholder: &str, value: Option<bool>, required: bool) -> Component {
    let option = |label: &str, value: &str, default: bool| SelectMenuOption {
        default,
        description: None,
        emoji: None,
        label: label.to_owned(),
        value: value.to_owned(),
    };
    Component::SelectMenu(SelectMenu {
        channel_types: None,
        custom_id: id,
        default_values: None,
        disabled: false,
        id: None,
        kind: SelectMenuType::Text,
        max_values: Some(1),
        min_values: Some(u8::from(required)),
        options: Some(vec![
            option(word::yes_no(true), "yes", value == Some(true)),
            option(word::yes_no(false), "no", value == Some(false)),
        ]),
        placeholder: Some(cut(placeholder, budget::SELECT_PLACEHOLDER)),
        required: Some(required),
    })
}

fn label(input: &Input) -> Component {
    let inner = self::input(encode_input_id(&input.id), &input.kind, input.required);
    Component::Label(Label {
        component: Box::new(inner),
        description: input
            .description
            .as_deref()
            .map(|d| cut(d, budget::INPUT_DESCRIPTION)),
        id: None,
        label: cut(&input.label, budget::INPUT_LABEL),
    })
}

/// Renders a modal. At most 5 inputs are kept (a rule's later fields are on
/// later pages, §11.6).
pub fn render_modal(modal: &Modal) -> RenderedModal {
    RenderedModal {
        custom_id: CustomId::from(&modal.action).encode(),
        title: cut(&modal.title, budget::MODAL_TITLE),
        components: modal
            .inputs
            .iter()
            .take(budget::MODAL_INPUTS)
            .map(label)
            .collect(),
    }
}
