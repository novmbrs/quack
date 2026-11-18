//! Interaction responses (§11.3): the initial callback (defer, message,
//! update, modal, autocomplete) and the follow-up webhooks (edit the
//! original response, create or edit a follow-up).
//!
//! An initial callback is not idempotent: a second one for the same
//! interaction is refused, so callers never repeat an ack.

use quack_core::ids::MessageId;
use serde::Serialize;
use twilight_model::channel::message::{Component, MessageFlags};

use super::messages::{AllowedMentions, Mentions};
use super::route::{Method, Route};
use super::{DiscordClient, Outcome, converted, with_json};
use crate::convert;
use crate::model::{InteractionRef, InteractionToken, SentMessage};
use crate::render::{RenderedMessage, RenderedModal};

/// One autocomplete suggestion: the label people see and the value sent
/// back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AutocompleteChoice {
    /// The label (at most 100 characters).
    pub name: String,
    /// The value (at most 100 characters).
    pub value: String,
}

#[derive(Serialize)]
struct Callback<'a> {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Data<'a>>,
}

#[derive(Default, Serialize)]
struct Data<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    components: Option<&'a [Component]>,
    /// A modal's components, already in their wire form
    /// ([`RenderedModal::wire_components`]); sent as `components`.
    #[serde(rename = "components", skip_serializing_if = "Option::is_none")]
    modal_components: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_mentions: Option<AllowedMentions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    choices: Option<&'a [AutocompleteChoice]>,
}

const EPHEMERAL: u64 = MessageFlags::EPHEMERAL.bits();
const COMPONENTS_V2: u64 = MessageFlags::IS_COMPONENTS_V2.bits();

/// The flags for a message carrying content.
const fn content_flags(ephemeral: bool) -> u64 {
    if ephemeral {
        COMPONENTS_V2 | EPHEMERAL
    } else {
        COMPONENTS_V2
    }
}

fn message_data(message: &RenderedMessage, flags: u64) -> Data<'_> {
    Data {
        components: Some(&message.components),
        flags: Some(flags),
        allowed_mentions: Some(AllowedMentions::of(&Mentions::None)),
        ..Data::default()
    }
}

impl DiscordClient {
    async fn callback(&self, interaction: &InteractionRef, callback: &Callback<'_>) -> Outcome<()> {
        self.unit(with_json(
            Route::interaction_callback(interaction.id, interaction.token.expose()),
            callback,
        ))
        .await
    }

    /// Defers a reply (type 5). `ephemeral` sets only the ephemeral flag;
    /// omit it in DMs, where Discord rejects it (§11.3).
    pub async fn defer(&self, interaction: &InteractionRef, ephemeral: bool) -> Outcome<()> {
        let data = ephemeral.then(|| Data {
            flags: Some(EPHEMERAL),
            ..Data::default()
        });
        self.callback(interaction, &Callback { kind: 5, data })
            .await
    }

    /// Defers an update of the message the component is on (type 6).
    pub async fn defer_update(&self, interaction: &InteractionRef) -> Outcome<()> {
        self.callback(
            interaction,
            &Callback {
                kind: 6,
                data: None,
            },
        )
        .await
    }

    /// Replies at once with a message (type 4), such as `/help`.
    pub async fn reply(
        &self,
        interaction: &InteractionRef,
        message: &RenderedMessage,
        ephemeral: bool,
    ) -> Outcome<()> {
        let data = message_data(message, content_flags(ephemeral));
        self.callback(
            interaction,
            &Callback {
                kind: 4,
                data: Some(data),
            },
        )
        .await
    }

    /// Replaces the message the component is on at once (type 7).
    pub async fn update_message(
        &self,
        interaction: &InteractionRef,
        message: &RenderedMessage,
    ) -> Outcome<()> {
        let data = message_data(message, COMPONENTS_V2);
        self.callback(
            interaction,
            &Callback {
                kind: 7,
                data: Some(data),
            },
        )
        .await
    }

    /// Opens a modal (type 9).
    pub async fn open_modal(
        &self,
        interaction: &InteractionRef,
        modal: &RenderedModal,
    ) -> Outcome<()> {
        let data = Data {
            modal_components: Some(modal.wire_components()),
            custom_id: Some(&modal.custom_id),
            title: Some(&modal.title),
            ..Data::default()
        };
        self.callback(
            interaction,
            &Callback {
                kind: 9,
                data: Some(data),
            },
        )
        .await
    }

    /// Answers an autocomplete request (type 8), at most 25 choices.
    pub async fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[AutocompleteChoice],
    ) -> Outcome<()> {
        let choices = choices.get(..choices.len().min(25)).unwrap_or_default();
        let data = Data {
            choices: Some(choices),
            ..Data::default()
        };
        self.callback(
            interaction,
            &Callback {
                kind: 8,
                data: Some(data),
            },
        )
        .await
    }

    /// Edits the original response after an ack (`PATCH
    /// /webhooks/{app}/{token}/messages/@original`); sets the Components V2
    /// flag (§11.3).
    pub async fn edit_interaction_original(
        &self,
        token: &InteractionToken,
        message: &RenderedMessage,
    ) -> Outcome<()> {
        let route = Route::webhook_message(
            Method::Patch,
            self.application_id(),
            token.expose(),
            "@original",
        );
        self.unit(with_json(route, &message_data(message, COMPONENTS_V2)))
            .await
    }

    /// Posts a follow-up message for an interaction.
    pub async fn followup(
        &self,
        token: &InteractionToken,
        message: &RenderedMessage,
        ephemeral: bool,
    ) -> Outcome<SentMessage> {
        let route = Route::followup(self.application_id(), token.expose());
        let body = message_data(message, content_flags(ephemeral));
        self.json::<twilight_model::channel::Message>(with_json(route, &body))
            .await
            .and_then(|m| {
                converted(convert::id(m.channel_id).zip(convert::id(m.id))).map(
                    |(channel_id, message_id)| SentMessage {
                        channel_id,
                        message_id,
                        attachments: Vec::new(),
                    },
                )
            })
    }

    /// Edits a follow-up message.
    pub async fn edit_followup(
        &self,
        token: &InteractionToken,
        message_id: MessageId,
        message: &RenderedMessage,
    ) -> Outcome<()> {
        let route = Route::webhook_message(
            Method::Patch,
            self.application_id(),
            token.expose(),
            &message_id.to_string(),
        );
        self.unit(with_json(route, &message_data(message, COMPONENTS_V2)))
            .await
    }
}
