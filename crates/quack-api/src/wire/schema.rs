//! OpenAPI schemas for `quack-core` values that wire types carry as they
//! are. Each enum schema is generated from the core enum's own `ALL` and
//! `as_str`, so the document can't drift from what is serialized. Fields
//! point here with `#[schema(value_type = schema::ActionKind)]`.

use std::borrow::Cow;

use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, OneOfBuilder, Schema, SchemaType, Type};
use utoipa::{PartialSchema, ToSchema};

/// Defines a schema-only marker type for a core `str_enum`.
macro_rules! enum_schema {
    ($($(#[$meta:meta])* $name:ident => $core:path;)+) => {
        $(
            $(#[$meta])*
            pub(crate) struct $name;

            impl PartialSchema for $name {
                fn schema() -> RefOr<Schema> {
                    ObjectBuilder::new()
                        .schema_type(SchemaType::Type(Type::String))
                        .enum_values(Some(<$core>::ALL.iter().map(|v| v.as_str())))
                        .into()
                }
            }

            impl ToSchema for $name {
                fn name() -> Cow<'static, str> {
                    Cow::Borrowed(stringify!($name))
                }
            }
        )+

        /// Every enum schema's name and allowed values (for tests).
        #[cfg(test)]
        pub(crate) fn enum_values() -> Vec<(&'static str, Vec<&'static str>)> {
            vec![$((stringify!($name), <$core>::ALL.iter().map(|v| v.as_str()).collect()),)+]
        }
    };
}

enum_schema! {
    /// What an action does.
    ActionKind => quack_core::action::ActionKind;
    /// Where an action stands.
    ActionState => quack_core::action::ActionState;
    /// Why an action failed.
    ActionFailure => quack_core::action::ActionFailure;
    /// Why an undo was left in place.
    ActionSkipReason => quack_core::action::ActionSkipReason;
    /// How one attempt ended.
    AttemptOutcome => quack_core::action::AttemptOutcome;
    /// Where a case came from.
    CaseSource => quack_core::case::CaseSource;
    /// Why a case was voided.
    VoidCause => quack_core::case::VoidCause;
    /// Where a DM stands.
    DmState => quack_core::dm::DmState;
    /// Why a DM failed.
    DmFailure => quack_core::dm::DmFailure;
    /// Where an appeal stands.
    AppealState => quack_core::appeal::AppealState;
    /// What happened on an appeal.
    AppealEventKind => quack_core::appeal::AppealEventKind;
    /// What an evidence item holds: a saved message or uploaded files.
    EvidenceKind => quack_core::evidence::EvidenceKind;
    /// Where an evidence item stands.
    EvidenceState => quack_core::evidence::EvidenceState;
    /// Why saving a message failed.
    EvidenceFailure => quack_core::evidence::EvidenceFailure;
    /// Where a file's copy stands.
    FileCopyState => quack_core::evidence::FileCopyState;
    /// What kind of answer a context field takes.
    FieldKind => quack_core::rule::FieldKind;
    /// A moderation log entry kind.
    ModLogKind => quack_core::modlog::ModLogKind;
    /// How a logged operation ended.
    ModLogResult => quack_core::modlog::ModLogResult;
    /// Where a logged operation came from.
    ModLogSource => quack_core::modlog::ModLogSource;
    /// Why Quack turned a module off.
    ModuleDisabledReason => quack_core::modules::ModuleDisabledReason;
    /// A Discord permission Quack checks.
    Permission => quack_core::permissions::Permission;
    /// Where a ticket stands.
    TicketState => quack_core::modules::TicketState;
    /// Why Quack closed a ticket itself.
    TicketCloseReason => quack_core::modules::TicketCloseReason;
    /// Why a transcript is partial.
    TranscriptPartial => quack_core::modules::TranscriptPartial;
    /// A server activity log event type.
    ActivityEvent => quack_core::modules::ActivityEvent;
}

/// A Discord ID: a decimal string (1–19 digits, below 2^63).
pub(crate) struct Snowflake;

impl PartialSchema for Snowflake {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(SchemaType::Type(Type::String))
            .pattern(Some("^[1-9][0-9]{0,18}$"))
            .examples([serde_json::json!("1100000000000000001")])
            .into()
    }
}

impl ToSchema for Snowflake {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("Snowflake")
    }
}

/// One case context answer: text (also message links and canonical
/// decimal numbers) or yes/no.
pub(crate) struct ContextValue;

impl PartialSchema for ContextValue {
    fn schema() -> RefOr<Schema> {
        OneOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::String)))
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::Boolean)))
            .into()
    }
}

impl ToSchema for ContextValue {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ContextValue")
    }
}

/// A submitted answer: like [`ContextValue`], and numbers as JSON numbers
/// or strings (both kept exactly).
pub(crate) struct ContextInput;

impl PartialSchema for ContextInput {
    fn schema() -> RefOr<Schema> {
        OneOfBuilder::new()
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::String)))
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::Boolean)))
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::Number)))
            .item(ObjectBuilder::new().schema_type(SchemaType::Type(Type::Null)))
            .into()
    }
}

impl ToSchema for ContextInput {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ContextInput")
    }
}

pub(crate) use rule::{Field, Level, RuleDefinition, RuleFile};

/// Schema mirrors of the rule definition document (§3.2). Requests and
/// responses carry `quack_core::rule` types directly (their serde is the
/// strict one); these only describe them. A test checks the mirrors'
/// properties against the core types' JSON.
#[expect(dead_code, reason = "schema-only mirrors; never constructed")]
mod rule {
    use utoipa::ToSchema;

    /// One rule version's full definition.
    #[derive(ToSchema)]
    pub(crate) struct RuleDefinition {
        /// 1–100 characters; unique among the server's active rules.
        name: String,
        /// Staff-facing explanation, up to 500 characters.
        description: String,
        /// The official reason shown to the member, 1–1,000 characters.
        reason: String,
        /// Whether cases under this version can be appealed.
        appealable: bool,
        /// Count only cases from the last N days (1–36,500); null counts all.
        counting_window_days: Option<u16>,
        /// Case context fields, in display order (at most 10).
        fields: Vec<Field>,
        /// Escalation levels, ascending by `from_case_count` (1–10).
        levels: Vec<Level>,
    }

    /// One case context field.
    #[derive(ToSchema)]
    pub(crate) struct Field {
        /// Stable machine key, `^[a-z0-9][a-z0-9_-]{1,63}$`.
        key: String,
        /// The question and the member's label, 1–100 characters.
        label: String,
        /// What kind of answer it takes.
        #[schema(value_type = super::FieldKind)]
        kind: (),
        /// Whether it must be answered.
        required: bool,
    }

    /// One rung of a rule.
    #[derive(ToSchema)]
    pub(crate) struct Level {
        /// The case count at which this level starts.
        from_case_count: u32,
        /// The level's action; null makes the case a warning.
        action: Option<LevelAction>,
        /// Whether the member gets a case DM.
        send_dm: bool,
    }

    /// The action a level takes.
    #[derive(ToSchema)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub(crate) enum LevelAction {
        /// Time the member out.
        Timeout {
            /// 1–2,419,200 seconds.
            duration_seconds: u32,
        },
        /// Remove the member from the server.
        Kick,
        /// Ban the member.
        Ban {
            /// Seconds of recent messages Discord deletes (0–604,800).
            delete_message_seconds: u32,
        },
    }

    /// An exported rule (§3.6).
    #[derive(ToSchema)]
    pub(crate) struct RuleFile {
        /// Always `quack-rule`.
        format: String,
        /// Always 1.
        format_version: u32,
        /// When it was exported.
        exported_at: String,
        /// The rule's definition.
        rule: RuleDefinition,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties<T: PartialSchema>() -> Vec<String> {
        let json = serde_json::to_value(T::schema()).unwrap_or_default();
        let mut keys: Vec<String> = json["properties"]
            .as_object()
            .map_or_else(Vec::new, |o| o.keys().cloned().collect());
        keys.sort();
        keys
    }

    fn keys(value: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = value
            .as_object()
            .map_or_else(Vec::new, |o| o.keys().cloned().collect());
        keys.sort();
        keys
    }

    #[test]
    fn rule_mirrors_match_core_json() {
        let def = quack_core::rule::starter_rule();
        let json = serde_json::to_value(&def).unwrap_or_default();
        assert_eq!(properties::<RuleDefinition>(), keys(&json));
        assert_eq!(properties::<Field>(), ["key", "kind", "label", "required"]);
        assert_eq!(properties::<Level>(), keys(&json["levels"][0]));
        let file = quack_core::rule::RuleFile::new(def, time::OffsetDateTime::UNIX_EPOCH);
        let file_json = serde_json::to_value(&file).unwrap_or_default();
        assert_eq!(properties::<RuleFile>(), keys(&file_json));
    }

    #[test]
    fn enum_schemas_list_every_value() {
        for (name, values) in enum_values() {
            assert!(!values.is_empty(), "{name}");
        }
        let json = serde_json::to_value(ActionKind::schema()).unwrap_or_default();
        assert_eq!(
            json["enum"],
            serde_json::json!(["timeout", "kick", "ban", "remove_timeout", "unban"])
        );
    }
}
