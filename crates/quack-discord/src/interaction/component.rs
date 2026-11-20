//! Buttons, selects, and modal submits.

use quack_core::ids::{CaseId, ChannelId, RuleId};
use twilight_model::application::interaction::InteractionDataResolved;
use twilight_model::application::interaction::message_component::MessageComponentInteractionData;
use twilight_model::application::interaction::modal::{
    ModalInteractionComponent, ModalInteractionData,
};
use twilight_model::channel::Message;
use twilight_model::channel::message::MessageFlags;

use super::{FieldValue, Request, SelectChoice, Source, upload};
use crate::convert;
use crate::custom_id::{ComponentId, CustomId, decode, decode_input_id};
use quack_text::actions::{InputId, SetupOp};

fn source(message: Option<&Message>) -> Source {
    message.map_or_else(Source::default, |m| Source {
        message_id: convert::id(m.id),
        ephemeral: m.flags.is_some_and(|f| f.contains(MessageFlags::EPHEMERAL)),
    })
}

/// Reads one select value in the context of its select.
fn choice(id: &ComponentId, value: &str) -> Option<SelectChoice> {
    match id {
        ComponentId::Current(CustomId::CasePicker | CustomId::AddAsEvidence { .. }) => {
            CaseId::from_u22(value).ok().map(SelectChoice::Case)
        }
        ComponentId::Current(CustomId::RulePicker(_) | CustomId::Setup(_, SetupOp::Rule)) => {
            RuleId::from_u22(value).ok().map(SelectChoice::Rule)
        }
        ComponentId::Current(CustomId::Setup(
            _,
            SetupOp::Channel | SetupOp::TicketEntry | SetupOp::TicketQueue,
        )) => value.parse::<ChannelId>().ok().map(SelectChoice::Channel),
        _ => None,
    }
}

pub(super) fn decode_component(
    data: &MessageComponentInteractionData,
    message: Option<&Message>,
) -> Request {
    let id = decode(&data.custom_id);
    let values = data.values.iter().filter_map(|v| choice(&id, v)).collect();
    Request::Component {
        id,
        values,
        source: source(message),
    }
}

/// Collects every input under labels and (older) action rows. A File
/// Upload's values are attachment ids; the files themselves are in the
/// modal data's `resolved.attachments`.
fn collect(
    components: &[ModalInteractionComponent],
    resolved: Option<&InteractionDataResolved>,
    out: &mut Vec<(InputId, FieldValue)>,
) {
    for component in components {
        match component {
            ModalInteractionComponent::Label(label) => {
                collect(std::slice::from_ref(&*label.component), resolved, out);
            }
            ModalInteractionComponent::ActionRow(row) => collect(&row.components, resolved, out),
            ModalInteractionComponent::FileUpload(input) => {
                if let Some(id) = decode_input_id(&input.custom_id) {
                    let files = input
                        .values
                        .iter()
                        .filter_map(|a| resolved?.attachments.get(a))
                        .filter_map(upload)
                        .collect();
                    out.push((id, FieldValue::Files(files)));
                }
            }
            ModalInteractionComponent::TextInput(input) => {
                if let Some(id) = decode_input_id(&input.custom_id) {
                    out.push((id, FieldValue::Text(input.value.clone())));
                }
            }
            ModalInteractionComponent::StringSelect(select) => {
                if let Some(id) = decode_input_id(&select.custom_id) {
                    let value = match select.values.first().map(String::as_str) {
                        Some("yes") => Some(true),
                        Some("no") => Some(false),
                        _ => None,
                    };
                    out.push((id, FieldValue::YesNo(value)));
                }
            }
            _ => {}
        }
    }
}

pub(super) fn decode_modal(data: &ModalInteractionData, message: Option<&Message>) -> Request {
    let mut fields = Vec::new();
    collect(&data.components, data.resolved.as_ref(), &mut fields);
    Request::ModalSubmit {
        id: decode(&data.custom_id),
        fields,
        source: source(message),
    }
}
