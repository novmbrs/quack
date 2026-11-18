//! A raw interaction → one typed [`Interaction`] of everything Quack
//! handles.
//!
//! Decoding never fails: anything Quack doesn't handle or can't read is
//! [`Request::Unknown`], so the router can still acknowledge it. The
//! invoker's roles and permissions are exactly what Discord put in the
//! payload (computed at the click, §2); the locale is ignored.

mod command;
mod component;

#[cfg(test)]
mod tests;

use quack_core::ids::{ChannelId, GuildId, MessageId, RoleId, RuleId};
use quack_core::permissions::{AccountFacts, Permissions};
use quack_telemetry::labels::{CommandName, InteractionKind};
use quack_text::actions::InputId;
use quack_text::help::Topic;
use time::OffsetDateTime;
use twilight_model::application::interaction::{
    Interaction as RawInteraction, InteractionData, InteractionType,
};

use crate::convert;
use crate::custom_id::ComponentId;
use crate::model::{InteractionRef, InteractionToken};

/// A member as an interaction payload describes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadMember {
    /// Their role ids.
    pub role_ids: Vec<RoleId>,
    /// Their permissions in the channel, as Discord computed them (absent
    /// for members resolved inside message payloads).
    pub permissions: Option<Permissions>,
    /// Their nickname.
    pub nick: Option<String>,
    /// When they joined.
    pub joined_at: Option<OffsetDateTime>,
    /// When their timeout ends.
    pub timeout_until: Option<OffsetDateTime>,
}

/// Someone named by an interaction: the invoker or a `USER` option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    /// Their account.
    pub account: AccountFacts,
    /// Their membership, when the payload had it (absent in DMs, and for a
    /// user option naming someone who is not in the server).
    pub member: Option<PayloadMember>,
}

/// The message an "Add case" message command was used on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetMessage {
    /// The message.
    pub id: MessageId,
    /// Its channel.
    pub channel_id: ChannelId,
    /// Its author, when the payload resolved it.
    pub author: Option<AccountFacts>,
}

/// A `rule` option: a picked rule's id, or typed text to match by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleArg {
    /// A rule chosen from autocomplete (its UUID).
    Id(RuleId),
    /// Text that was not a picked rule (matched by exact name, §11.1).
    Name(String),
}

/// Which command an autocomplete request is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutocompleteFor {
    /// `/case add` (active rules).
    CaseAdd,
    /// `/case list` (active and archived rules).
    CaseList,
}

/// One chosen value of a select, read with its select's custom id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectChoice {
    /// A case (the case picker).
    Case(quack_core::ids::CaseId),
    /// A rule (the rule picker, the honeypot rule select).
    Rule(RuleId),
    /// A channel (setup channel selects).
    Channel(ChannelId),
}

/// One submitted modal value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    /// A text input's value (possibly empty).
    Text(String),
    /// A Yes / No select: `None` when nothing was chosen.
    YesNo(Option<bool>),
}

/// Where a component or modal interaction came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// The message the component was on (or the modal opened from).
    pub message_id: Option<MessageId>,
    /// Whether that message was ephemeral (decides deferred update vs a new
    /// ephemeral message, §11.3).
    pub ephemeral: bool,
}

/// Why an interaction decoded to [`Request::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    /// A ping or another type Quack doesn't handle.
    UnsupportedType,
    /// The payload had no data.
    MissingData,
    /// A command, subcommand, or option set Quack didn't register.
    UnknownCommand,
    /// The payload had no user.
    MissingUser,
}

/// Everything Quack handles, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `/case add member rule [private]`.
    CaseAdd {
        /// The member.
        member: Person,
        /// The rule.
        rule: RuleArg,
        /// Whether the receipt should be private.
        private: Option<bool>,
    },
    /// `/case view number`.
    CaseView {
        /// The case number.
        number: u64,
    },
    /// `/case list [member] [rule]`.
    CaseList {
        /// The member filter.
        member: Option<Person>,
        /// The rule filter.
        rule: Option<RuleArg>,
    },
    /// `/case void number reason`.
    CaseVoid {
        /// The case number.
        number: u64,
        /// Why.
        reason: String,
    },
    /// `/case history member`.
    CaseHistory {
        /// The member.
        member: Person,
    },
    /// `/appeals`.
    Appeals,
    /// `/rules`.
    Rules,
    /// `/setup`.
    Setup,
    /// `/help [topic]`.
    Help {
        /// The topic.
        topic: Option<Topic>,
    },
    /// The "Add case" message command.
    AddCaseFromMessage {
        /// The message.
        message: TargetMessage,
    },
    /// The "Add case" member command.
    AddCaseForMember {
        /// The member.
        member: Person,
    },
    /// Autocomplete for a `rule` option.
    Autocomplete {
        /// Which command.
        command: AutocompleteFor,
        /// What was typed so far.
        query: String,
    },
    /// A button or select.
    Component {
        /// Its custom id.
        id: ComponentId,
        /// The chosen values of a select.
        values: Vec<SelectChoice>,
        /// Where it was.
        source: Source,
    },
    /// A submitted modal.
    ModalSubmit {
        /// Its custom id.
        id: ComponentId,
        /// The values, by input.
        fields: Vec<(InputId, FieldValue)>,
        /// Where the modal was opened from.
        source: Source,
    },
    /// Anything else; acknowledge it with a generic answer.
    Unknown(UnknownReason),
}

/// One decoded interaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interaction {
    /// The interaction id.
    pub id: u64,
    /// Its token, for responses.
    pub token: InteractionToken,
    /// The server (absent in DMs).
    pub guild_id: Option<GuildId>,
    /// The channel it happened in.
    pub channel_id: Option<ChannelId>,
    /// Who did it; `None` only for malformed payloads (then `request` is
    /// `Unknown`).
    pub invoker: Option<Person>,
    /// Quack's own permissions in the channel, as Discord computed them.
    pub app_permissions: Option<Permissions>,
    /// What was asked.
    pub request: Request,
}

/// Discord's epoch (2015-01-01) in Unix milliseconds.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

impl Interaction {
    /// What responses need: the id and token.
    pub fn reference(&self) -> InteractionRef {
        InteractionRef {
            id: self.id,
            token: self.token.clone(),
        }
    }

    /// When Discord created the interaction (from its snowflake), for the
    /// 2.5-second ack deadline (§11.3).
    pub fn created_at(&self) -> Option<OffsetDateTime> {
        let ms = (self.id >> 22) + DISCORD_EPOCH_MS;
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
    }

    /// The interaction kind recorded on metrics and spans.
    pub const fn telemetry_kind(&self) -> InteractionKind {
        match &self.request {
            Request::AddCaseFromMessage { .. } => InteractionKind::MessageCommand,
            Request::AddCaseForMember { .. } => InteractionKind::UserCommand,
            Request::Autocomplete { .. } => InteractionKind::Autocomplete,
            Request::Component { .. } => InteractionKind::Component,
            Request::ModalSubmit { .. } => InteractionKind::Modal,
            _ => InteractionKind::SlashCommand,
        }
    }

    /// The command name recorded on metrics (a static name; components and
    /// modals use their custom-id code family).
    pub const fn command_name(&self) -> CommandName {
        CommandName::new(match &self.request {
            Request::CaseAdd { .. }
            | Request::Autocomplete {
                command: AutocompleteFor::CaseAdd,
                ..
            } => "case add",
            Request::CaseView { .. } => "case view",
            Request::CaseList { .. }
            | Request::Autocomplete {
                command: AutocompleteFor::CaseList,
                ..
            } => "case list",
            Request::CaseVoid { .. } => "case void",
            Request::CaseHistory { .. } => "case history",
            Request::Appeals => "appeals",
            Request::Rules => "rules",
            Request::Setup => "setup",
            Request::Help { .. } => "help",
            Request::AddCaseFromMessage { .. } | Request::AddCaseForMember { .. } => "Add case",
            Request::Component { .. } => "component",
            Request::ModalSubmit { .. } => "modal",
            Request::Unknown(_) => "unknown",
        })
    }
}

fn payload_member(m: &twilight_model::guild::PartialMember) -> PayloadMember {
    PayloadMember {
        role_ids: m.roles.iter().filter_map(|r| convert::id(*r)).collect(),
        permissions: m.permissions.map(convert::permissions),
        nick: m.nick.clone(),
        joined_at: m.joined_at.and_then(convert::time),
        timeout_until: m.communication_disabled_until.and_then(convert::time),
    }
}

fn invoker(raw: &RawInteraction) -> Option<Person> {
    let user = raw
        .member
        .as_ref()
        .and_then(|m| m.user.as_ref())
        .or(raw.user.as_ref())?;
    Some(Person {
        account: convert::user(user)?,
        member: raw.member.as_ref().map(payload_member),
    })
}

/// Decodes a raw interaction. Never fails.
pub fn decode(raw: &RawInteraction) -> Interaction {
    let invoker = invoker(raw);
    let request = if invoker.is_none() {
        Request::Unknown(UnknownReason::MissingUser)
    } else {
        match (raw.kind, &raw.data) {
            (
                InteractionType::ApplicationCommand | InteractionType::ApplicationCommandAutocomplete,
                Some(InteractionData::ApplicationCommand(data)),
            ) => command::decode(data, raw.kind == InteractionType::ApplicationCommandAutocomplete),
            (InteractionType::MessageComponent, Some(InteractionData::MessageComponent(data))) => {
                component::decode_component(data, raw.message.as_ref())
            }
            (InteractionType::ModalSubmit, Some(InteractionData::ModalSubmit(data))) => {
                component::decode_modal(data, raw.message.as_ref())
            }
            (
                InteractionType::ApplicationCommand
                | InteractionType::ApplicationCommandAutocomplete
                | InteractionType::MessageComponent
                | InteractionType::ModalSubmit,
                _,
            ) => Request::Unknown(UnknownReason::MissingData),
            _ => Request::Unknown(UnknownReason::UnsupportedType),
        }
    };
    #[allow(deprecated, reason = "older payloads only carry channel_id")]
    let channel = raw.channel.as_ref().map(|c| c.id).or(raw.channel_id);
    Interaction {
        id: raw.id.get(),
        token: InteractionToken::new(raw.token.clone()),
        guild_id: raw.guild_id.and_then(convert::id),
        channel_id: channel.and_then(convert::id),
        invoker,
        app_permissions: raw.app_permissions.map(convert::permissions),
        request,
    }
}

/// Decodes an `INTERACTION_CREATE` payload's JSON (the `d` object). A
/// payload Twilight can't read gives `None`; nothing can be acknowledged
/// without its id and token.
pub fn decode_json(json: &str) -> Option<Interaction> {
    serde_json::from_str::<RawInteraction>(json)
        .ok()
        .map(|raw| decode(&raw))
}
