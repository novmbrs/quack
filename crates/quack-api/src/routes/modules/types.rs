//! Wire types of the module routes (§14.3). `ModuleState` is built here
//! from the engine; the module-specific types are filled by the module
//! crates once they are wired (see the parent module).

use quack_core::ids::{ChannelId, MemberId, RuleId, TicketId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::error::Problem;
use crate::wire::{schema, ts};

/// Whether a module is on, why Quack turned it off, and what blocks it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct ModuleState {
    /// It is on.
    pub enabled: bool,
    /// Why Quack turned it off (`channel_deleted`, `quack_left`).
    #[schema(value_type = Option<schema::ModuleDisabledReason>)]
    pub disabled_reason: Option<quack_core::modules::ModuleDisabledReason>,
    /// What blocks turning it on (empty when nothing is known to).
    pub problems: Vec<Problem>,
}

impl From<quack_engine::settings::ModuleState> for ModuleState {
    fn from(s: quack_engine::settings::ModuleState) -> Self {
        Self {
            enabled: s.enabled,
            disabled_reason: s.disabled_reason,
            problems: Vec::new(),
        }
    }
}

/// Each module's state, by JSON module name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
pub(crate) struct ModulesState {
    /// Tickets.
    pub tickets: ModuleState,
    /// The server activity log.
    pub activity_log: ModuleState,
    /// The honeypot.
    pub honeypot: ModuleState,
}

impl From<quack_engine::settings::ModulesView> for ModulesState {
    fn from(m: quack_engine::settings::ModulesView) -> Self {
        Self {
            tickets: m.tickets.into(),
            activity_log: m.activity_log.into(),
            honeypot: m.honeypot.into(),
        }
    }
}

/// Route 42's body: the settings and the version they were read at.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TicketSettingsUpdate {
    /// The `version` the page was read at; a newer one is refused with
    /// `settings_changed`.
    pub expected_version: u64,
    /// Where members open tickets.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub entry_channel_id: Option<ChannelId>,
    /// The staff channel new tickets are announced in.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub queue_channel_id: Option<ChannelId>,
    /// Days transcripts are kept (1–365).
    pub transcript_retention_days: u16,
}

/// Ticket settings (part of route 41).
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TicketSettings {
    /// Where members open tickets.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub entry_channel_id: Option<ChannelId>,
    /// The staff channel new tickets are announced in.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub queue_channel_id: Option<ChannelId>,
    /// Days transcripts are kept (1–365).
    pub transcript_retention_days: u16,
}

/// The tickets module (route 41).
#[allow(dead_code, reason = "filled once quack-tickets is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TicketsModule {
    #[serde(flatten)]
    pub state: ModuleState,
    /// The settings' version: send it back as `expected_version`.
    pub version: u64,
    pub settings: TicketSettings,
    /// Tickets open now.
    pub open_tickets: u64,
}

/// Route 44's body: the settings and the version they were read at.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityLogSettingsUpdate {
    /// The `version` the page was read at; a newer one is refused with
    /// `settings_changed`.
    pub expected_version: u64,
    /// The channel each event type posts to (absent: not logged).
    #[schema(value_type = std::collections::HashMap<schema::ActivityEvent, schema::Snowflake>)]
    pub routes: std::collections::BTreeMap<quack_core::modules::ActivityEvent, ChannelId>,
    /// Include message text.
    pub include_message_content: bool,
    /// Include file names.
    pub include_file_names: bool,
    /// Include embed types.
    pub include_embed_types: bool,
}

/// Server activity log settings (part of route 43).
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityLogSettings {
    /// The channel each event type posts to (absent: not logged).
    #[schema(value_type = std::collections::HashMap<schema::ActivityEvent, schema::Snowflake>)]
    pub routes: std::collections::BTreeMap<quack_core::modules::ActivityEvent, ChannelId>,
    /// Include message text.
    pub include_message_content: bool,
    /// Include file names.
    pub include_file_names: bool,
    /// Include embed types.
    pub include_embed_types: bool,
}

/// The server activity log module (route 43). Counters are in-process
/// and reset when Quack restarts.
#[allow(dead_code, reason = "filled once quack-activity-log is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct ActivityLogModule {
    #[serde(flatten)]
    pub state: ModuleState,
    /// The settings' version: send it back as `expected_version`.
    pub version: u64,
    pub settings: ActivityLogSettings,
    /// Events posted since Quack started.
    pub posted_since_start: u64,
    /// Events dropped since Quack started.
    pub dropped_since_start: u64,
}

/// Route 46's body: the settings and the version they were read at.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HoneypotSettingsUpdate {
    /// The `version` the page was read at; a newer one is refused with
    /// `settings_changed`.
    pub expected_version: u64,
    /// The honeypot channel.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub channel_id: Option<ChannelId>,
    /// The rule applied to members who post there.
    #[schema(value_type = Option<uuid::Uuid>)]
    pub rule_id: Option<RuleId>,
    /// The warning posted in the channel (up to 1,500 characters).
    pub warning_text: Option<String>,
}

/// Honeypot settings (part of route 45).
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HoneypotSettings {
    /// The honeypot channel.
    #[schema(value_type = Option<schema::Snowflake>)]
    pub channel_id: Option<ChannelId>,
    /// The rule applied to members who post there.
    #[schema(value_type = Option<uuid::Uuid>)]
    pub rule_id: Option<RuleId>,
    /// The warning posted in the channel (up to 1,500 characters).
    pub warning_text: Option<String>,
}

/// A rule the honeypot may use.
#[allow(dead_code, reason = "filled once quack-honeypot is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct EligibleRule {
    #[schema(value_type = uuid::Uuid)]
    pub id: RuleId,
    pub name: String,
    /// Its levels, for the warning's default wording (anyone who can read
    /// the honeypot settings sees them, without reading rules).
    #[schema(value_type = Vec<schema::Level>)]
    pub levels: Vec<quack_core::rule::Level>,
}

/// The honeypot module (route 45).
#[allow(dead_code, reason = "filled once quack-honeypot is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct HoneypotModule {
    #[serde(flatten)]
    pub state: ModuleState,
    /// The settings' version: send it back as `expected_version`.
    pub version: u64,
    pub settings: HoneypotSettings,
    /// Active rules without required fields.
    pub eligible_rules: Vec<EligibleRule>,
    pub cases_created: u64,
    pub pending_hits: u64,
    pub failed_hits: u64,
}

/// A ticket's transcript, in brief.
#[allow(dead_code, reason = "filled once quack-tickets is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TicketTranscriptInfo {
    /// `available` or `expired`.
    pub state: String,
    pub complete: bool,
    #[schema(value_type = Option<schema::TranscriptPartial>)]
    pub partial_cause: Option<quack_core::modules::TranscriptPartial>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: OffsetDateTime,
}

/// A ticket. The ticket's member gets it without `closed_by` and with
/// `closed_by_you` instead.
#[allow(dead_code, reason = "filled once quack-tickets is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Ticket {
    #[schema(value_type = uuid::Uuid)]
    pub id: TicketId,
    #[schema(value_type = schema::Snowflake)]
    pub member_id: MemberId,
    #[schema(value_type = schema::TicketState)]
    pub state: quack_core::modules::TicketState,
    #[schema(value_type = Option<schema::Snowflake>)]
    pub thread_id: Option<ChannelId>,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub opened_at: Option<OffsetDateTime>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub close_requested_at: Option<OffsetDateTime>,
    #[serde(with = "ts::option")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub closed_at: Option<OffsetDateTime>,
    /// Staff projection only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<schema::Snowflake>)]
    pub closed_by: Option<quack_core::ids::AccountId>,
    /// Member projection only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_by_you: Option<bool>,
    #[schema(value_type = Option<schema::TicketCloseReason>)]
    pub close_reason: Option<quack_core::modules::TicketCloseReason>,
    pub thread_delete_failed: bool,
    pub transcript: Option<TicketTranscriptInfo>,
}

/// A page of tickets (route 47).
#[allow(dead_code, reason = "filled once quack-tickets is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct TicketPage {
    pub tickets: Vec<Ticket>,
    pub next_cursor: Option<String>,
}

/// A ticket's transcript (route 49).
#[allow(dead_code, reason = "filled once quack-tickets is wired")]
#[derive(Clone, Debug, Serialize, ToSchema)]
pub(crate) struct Transcript {
    pub content: String,
    pub complete: bool,
    #[serde(with = "ts")]
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: OffsetDateTime,
}
