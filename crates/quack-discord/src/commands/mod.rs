//! Application command definitions (§11.1) built from `quack_text::command`,
//! and their registration (§11.2).
//!
//! Every command is guild-install only (`integration_types = [0]`,
//! `contexts = [0]`). Only `/setup` has a default member permission
//! (Manage Server): staff may hold a role rather than a Discord permission,
//! so Quack checks capabilities itself.

mod sync;

#[cfg(test)]
mod tests;

use quack_text::command::{self as text, Named};
use twilight_model::application::command::{Command, CommandType};
use twilight_model::application::interaction::InteractionContextType;
use twilight_model::guild::Permissions;
use twilight_model::oauth::ApplicationIntegrationType;
use twilight_util::builder::command::{
    BooleanBuilder, CommandBuilder, IntegerBuilder, StringBuilder, SubCommandBuilder, UserBuilder,
};

pub use sync::{Scope, SyncError, SyncReport, canonical, command_ids, for_scope, sync};

/// The most characters a `rule` option accepts (§11.1).
const RULE_OPTION_MAX: u16 = 100;
/// The longest void reason (§3.9).
const VOID_REASON_MAX: u16 = 1_000;

fn command(named: Named, kind: CommandType) -> CommandBuilder {
    CommandBuilder::new(named.name, named.description, kind)
        .contexts([InteractionContextType::Guild])
        .integration_types([ApplicationIntegrationType::GuildInstall])
}

fn context_menu(kind: CommandType) -> Command {
    CommandBuilder::new(text::ADD_CASE, "", kind)
        .contexts([InteractionContextType::Guild])
        .integration_types([ApplicationIntegrationType::GuildInstall])
        .build()
}

fn sub(named: Named) -> SubCommandBuilder {
    SubCommandBuilder::new(named.name, named.description)
}

fn user(named: Named, required: bool) -> UserBuilder {
    UserBuilder::new(named.name, named.description).required(required)
}

fn number(named: Named) -> IntegerBuilder {
    IntegerBuilder::new(named.name, named.description)
        .required(true)
        .min_value(1)
}

fn rule(named: Named, required: bool) -> StringBuilder {
    StringBuilder::new(named.name, named.description)
        .required(required)
        .autocomplete(true)
}

/// `/case` and its five subcommands.
fn case() -> Command {
    command(text::CASE, CommandType::ChatInput)
        .option(
            sub(text::CASE_ADD)
                .option(user(text::CASE_ADD_MEMBER, true))
                .option(rule(text::CASE_ADD_RULE, true).max_length(RULE_OPTION_MAX))
                .option(
                    BooleanBuilder::new(
                        text::CASE_ADD_PRIVATE.name,
                        text::CASE_ADD_PRIVATE.description,
                    )
                    .required(false),
                ),
        )
        .option(sub(text::CASE_VIEW).option(number(text::CASE_NUMBER)))
        .option(
            sub(text::CASE_LIST)
                .option(user(text::CASE_LIST_MEMBER, false))
                .option(rule(text::CASE_LIST_RULE, false)),
        )
        .option(
            sub(text::CASE_VOID)
                .option(number(text::CASE_NUMBER))
                .option(
                    StringBuilder::new(
                        text::CASE_VOID_REASON.name,
                        text::CASE_VOID_REASON.description,
                    )
                    .required(true)
                    .min_length(1)
                    .max_length(VOID_REASON_MAX),
                ),
        )
        .option(sub(text::CASE_HISTORY).option(user(text::CASE_HISTORY_MEMBER, true)))
        .build()
}

/// The ops server's `/ops` (§21.8): hidden from everyone but
/// administrators by default; Quack checks the operator list itself.
pub fn ops_definitions() -> Vec<Command> {
    vec![
        command(text::OPS, CommandType::ChatInput)
            .default_member_permissions(Permissions::empty())
            .option(sub(text::OPS_STATUS))
            .option(
                sub(text::OPS_SERVER).option(
                    StringBuilder::new(text::OPS_SERVER_ID.name, text::OPS_SERVER_ID.description)
                        .required(true)
                        .min_length(1)
                        .max_length(20),
                ),
            )
            .option(sub(text::OPS_JOBS))
            .build(),
    ]
}

/// Every command Quack registers, in a stable order.
pub fn definitions() -> Vec<Command> {
    vec![
        case(),
        command(text::APPEALS, CommandType::ChatInput).build(),
        command(text::RULES, CommandType::ChatInput).build(),
        command(text::SETUP, CommandType::ChatInput)
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .build(),
        command(text::HELP, CommandType::ChatInput)
            .option(
                StringBuilder::new(text::HELP_TOPIC.name, text::HELP_TOPIC.description)
                    .required(false)
                    .choices(text::HELP_TOPICS.iter().map(|c| (c.name, c.value))),
            )
            .build(),
        context_menu(CommandType::Message),
        context_menu(CommandType::User),
    ]
}
