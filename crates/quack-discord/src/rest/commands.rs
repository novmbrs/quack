//! Listing and overwriting application commands (§11.2).

use quack_core::ids::GuildId;
use serde::Deserialize;
use twilight_model::application::command::Command;

use super::route::{Method, Route};
use super::transport::Request;
use super::{DiscordClient, Outcome, with_json};

/// One command as Discord has it registered.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredCommand {
    /// Its id (for `</name:id>` mentions).
    pub id: u64,
    /// Its type (1 slash, 2 member, 3 message).
    pub kind: u8,
    /// Its name.
    pub name: String,
    /// The whole definition as Discord returned it.
    pub definition: serde_json::Value,
}

#[derive(Deserialize)]
struct Head {
    id: String,
    #[serde(rename = "type", default = "chat_input")]
    kind: u8,
    name: String,
}

const fn chat_input() -> u8 {
    1
}

fn registered(list: Vec<serde_json::Value>) -> Vec<RegisteredCommand> {
    list.into_iter()
        .filter_map(|definition| {
            let head = Head::deserialize(&definition).ok()?;
            Some(RegisteredCommand {
                id: head.id.parse().ok()?,
                kind: head.kind,
                name: head.name,
                definition,
            })
        })
        .collect()
}

impl DiscordClient {
    /// The registered commands: global, or one server's when `guild` is set.
    pub async fn registered_commands(
        &self,
        guild: Option<GuildId>,
    ) -> Outcome<Vec<RegisteredCommand>> {
        self.json::<Vec<serde_json::Value>>(Request::new(Route::commands(
            Method::Get,
            self.application_id(),
            guild,
        )))
        .await
        .map(registered)
    }

    /// Replaces every registered command with `commands` (one bulk
    /// overwrite, which also removes commands not listed).
    pub async fn overwrite_commands(
        &self,
        guild: Option<GuildId>,
        commands: &[Command],
    ) -> Outcome<Vec<RegisteredCommand>> {
        let route = Route::commands(Method::Put, self.application_id(), guild);
        self.json::<Vec<serde_json::Value>>(with_json(route, &commands))
            .await
            .map(registered)
    }
}
