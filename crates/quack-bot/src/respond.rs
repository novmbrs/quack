//! Answering interactions: the [`Responder`] (initial callbacks, edits of
//! the original response, follow-ups) and its Discord implementation.
//!
//! Handlers never call Discord's interaction endpoints directly; they go
//! through the ack helper (`crate::ack`), which uses a `Responder`. Tests
//! supply one that records every answer.

use std::sync::{Arc, PoisonError, RwLock};

use quack_discord::DiscordClient;
use quack_discord::Outcome;
use quack_discord::model::{InteractionRef, InteractionToken, OutgoingFile};
use quack_discord::render::{render, render_modal};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::BoxFuture;
use quack_text::model::Modal;
use quack_text::{CommandIds, Msg};

/// Everything Quack sends in answer to an interaction (§11.3).
///
/// Implemented over Discord by [`DiscordResponder`] and by a recorder in
/// tests. An initial callback (`defer`, `defer_update`, `reply`, `modal`,
/// `autocomplete`) is sent at most once per interaction.
pub trait Responder: Send + Sync + 'static {
    /// Type 5: a deferred message; `ephemeral` sets only the flag.
    fn defer<'a>(&'a self, ix: &'a InteractionRef, ephemeral: bool) -> BoxFuture<'a, Outcome<()>>;
    /// Type 6: a deferred update of the message the component is on.
    fn defer_update<'a>(&'a self, ix: &'a InteractionRef) -> BoxFuture<'a, Outcome<()>>;
    /// Type 4: an immediate message.
    fn reply<'a>(
        &'a self,
        ix: &'a InteractionRef,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Type 9: a modal.
    fn modal<'a>(&'a self, ix: &'a InteractionRef, modal: &'a Modal) -> BoxFuture<'a, Outcome<()>>;
    /// Type 8: autocomplete choices.
    fn autocomplete<'a>(
        &'a self,
        ix: &'a InteractionRef,
        choices: &'a [AutocompleteChoice],
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Edits the original response after an ack; `files` (the images an
    /// ops card shows) replace its attachments when there are any.
    fn edit_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
    ) -> BoxFuture<'a, Outcome<()>>;
    /// Posts a follow-up message.
    fn followup<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>>;
}

/// The command ids from the last command sync (§11.2), shared by every
/// renderer so command mentions are clickable once they are known.
#[derive(Clone, Debug, Default)]
pub struct CommandMentions(Arc<RwLock<CommandIds>>);

impl CommandMentions {
    /// Stores the ids a sync returned.
    pub fn set(&self, ids: CommandIds) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = ids;
    }

    /// The current ids (empty before the first sync).
    pub fn get(&self) -> CommandIds {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The [`Responder`] over Quack's Discord REST client: renders `Msg` to
/// Components V2 and modals with `quack_discord::render`.
#[derive(Clone, Debug)]
pub struct DiscordResponder {
    client: DiscordClient,
    commands: CommandMentions,
}

impl DiscordResponder {
    /// A responder over `client`, rendering command mentions from
    /// `commands`.
    pub const fn new(client: DiscordClient, commands: CommandMentions) -> Self {
        Self { client, commands }
    }
}

impl Responder for DiscordResponder {
    fn defer<'a>(&'a self, ix: &'a InteractionRef, ephemeral: bool) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(self.client.defer(ix, ephemeral))
    }

    fn defer_update<'a>(&'a self, ix: &'a InteractionRef) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(self.client.defer_update(ix))
    }

    fn reply<'a>(
        &'a self,
        ix: &'a InteractionRef,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            let rendered = render(msg, &self.commands.get());
            self.client.reply(ix, &rendered, ephemeral).await
        })
    }

    fn modal<'a>(&'a self, ix: &'a InteractionRef, modal: &'a Modal) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move { self.client.open_modal(ix, &render_modal(modal)).await })
    }

    fn autocomplete<'a>(
        &'a self,
        ix: &'a InteractionRef,
        choices: &'a [AutocompleteChoice],
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(self.client.autocomplete(ix, choices))
    }

    fn edit_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            let rendered = render(msg, &self.commands.get());
            self.client
                .edit_interaction_original(token, &rendered, files)
                .await
        })
    }

    fn followup<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        Box::pin(async move {
            let rendered = render(msg, &self.commands.get());
            self.client
                .followup(token, &rendered, ephemeral)
                .await
                .map(|_| ())
        })
    }
}
