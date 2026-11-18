//! Registering commands only when they changed (§11.2).
//!
//! Both sides are canonicalized (ids, versions, and default fields dropped;
//! keys compared as maps; option order kept) and compared as sets ordered by
//! type and name. Equal → keep the remote list. Different → one bulk
//! overwrite with every definition, which also removes Go-era commands.

use quack_core::ids::GuildId;
use quack_text::CommandIds;
use quack_text::model::Command as TextCommand;
use serde_json::{Map, Value};
use twilight_model::application::command::Command;

use crate::rest::{DiscordClient, Outcome, RegisteredCommand};

/// Where commands are registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every server (production).
    Global,
    /// One development server (`QUACK_DEV_GUILD_ID`).
    Guild(GuildId),
}

impl Scope {
    const fn guild(self) -> Option<GuildId> {
        match self {
            Self::Global => None,
            Self::Guild(g) => Some(g),
        }
    }
}

/// What a sync did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    /// Whether the commands were overwritten.
    pub changed: bool,
    /// The registered ids, for command mentions in text.
    pub ids: CommandIds,
}

/// Why a sync failed. `serve` stops and `commands sync` exits 1 (§11.2).
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Reading the registered commands failed.
    #[error("couldn't read the registered commands from Discord: {0}")]
    List(String),
    /// The bulk overwrite failed.
    #[error("couldn't register the commands with Discord: {0}")]
    Overwrite(String),
}

/// Fields Discord adds or Quack doesn't set.
const DROPPED: [&str; 7] = [
    "id",
    "application_id",
    "version",
    "guild_id",
    "dm_permission",
    "name_localized",
    "description_localized",
];

fn is_default(key: &str, value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(false) => matches!(key, "required" | "autocomplete" | "nsfw"),
        Value::Array(a) => a.is_empty() && matches!(key, "options" | "choices"),
        Value::Object(o) => {
            o.is_empty() && matches!(key, "name_localizations" | "description_localizations")
        }
        _ => false,
    }
}

/// A definition without ids, versions, or default fields.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                if DROPPED.contains(&key.as_str()) {
                    continue;
                }
                let value = canonical(value);
                if !is_default(key, &value) {
                    out.insert(key.clone(), value);
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn sort_key(v: &Value) -> (u64, String) {
    (
        v.get("type").and_then(Value::as_u64).unwrap_or(1),
        v.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

fn canonical_set(values: impl Iterator<Item = Value>) -> Vec<Value> {
    let mut list: Vec<Value> = values.map(|v| canonical(&v)).collect();
    list.sort_by_key(sort_key);
    list
}

/// Whether the registered commands already match the definitions.
pub(crate) fn matches(local: &[Command], remote: &[RegisteredCommand]) -> bool {
    let local = canonical_set(local.iter().filter_map(|c| serde_json::to_value(c).ok()));
    let remote = canonical_set(remote.iter().map(|r| r.definition.clone()));
    local == remote
}

/// The ids of Quack's slash commands, keyed for `quack_text` mentions.
/// Subcommands share their parent's id (`</case view:ID>`).
pub fn command_ids(registered: &[RegisteredCommand]) -> CommandIds {
    const ALL: [TextCommand; 9] = [
        TextCommand::CaseAdd,
        TextCommand::CaseView,
        TextCommand::CaseList,
        TextCommand::CaseVoid,
        TextCommand::CaseHistory,
        TextCommand::Appeals,
        TextCommand::Rules,
        TextCommand::Setup,
        TextCommand::Help,
    ];
    CommandIds(
        ALL.iter()
            .filter_map(|c| {
                registered
                    .iter()
                    .find(|r| r.kind == 1 && r.name == c.top())
                    .map(|r| (*c, r.id))
            })
            .collect(),
    )
}

fn failure<T>(outcome: &Outcome<T>) -> String {
    match outcome {
        Outcome::Done(_) => "done".to_owned(),
        Outcome::Refused(r) => format!("refused ({})", r.machine_code()),
        Outcome::RetryAfter(d) => format!("rate limited for {d:?}"),
        Outcome::NotSent(c) => format!("not sent ({c:?})"),
        Outcome::Unknown(c) => format!("no answer ({c:?})"),
    }
}

/// Registers `definitions` in `scope` when they differ from what Discord
/// has, and returns the command ids either way.
pub async fn sync(
    client: &DiscordClient,
    scope: Scope,
    definitions: &[Command],
) -> Result<SyncReport, SyncError> {
    let remote = match client.registered_commands(scope.guild()).await {
        Outcome::Done(remote) => remote,
        other => return Err(SyncError::List(failure(&other))),
    };
    if matches(definitions, &remote) {
        return Ok(SyncReport {
            changed: false,
            ids: command_ids(&remote),
        });
    }
    match client.overwrite_commands(scope.guild(), definitions).await {
        Outcome::Done(registered) => {
            tracing::info!(count = registered.len(), "registered commands");
            Ok(SyncReport {
                changed: true,
                ids: command_ids(&registered),
            })
        }
        other => Err(SyncError::Overwrite(failure(&other))),
    }
}
