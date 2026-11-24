//! Slash, message, and member commands, and autocomplete.

use quack_core::ids::RuleId;
use quack_text::help::Topic;
use twilight_model::application::command::CommandType;
use twilight_model::application::interaction::InteractionDataResolved;
use twilight_model::application::interaction::application_command::{
    CommandData, CommandDataOption, CommandOptionValue,
};
use twilight_model::id::Id;
use twilight_model::id::marker::UserMarker;

use super::{
    AutocompleteFor, OpsRequest, PayloadMember, Person, Request, RuleArg, TargetMessage,
    UnknownReason, Upload, upload,
};
use crate::convert;

const UNKNOWN: Request = Request::Unknown(UnknownReason::UnknownCommand);

fn option<'a>(options: &'a [CommandDataOption], name: &str) -> Option<&'a CommandOptionValue> {
    options.iter().find(|o| o.name == name).map(|o| &o.value)
}

fn person(resolved: Option<&InteractionDataResolved>, id: Id<UserMarker>) -> Option<Person> {
    let resolved = resolved?;
    let account = convert::user(resolved.users.get(&id)?)?;
    let member = resolved.members.get(&id).map(|m| PayloadMember {
        role_ids: m.roles.iter().filter_map(|r| convert::id(*r)).collect(),
        permissions: Some(convert::permissions(m.permissions)),
        nick: m.nick.clone(),
        joined_at: m.joined_at.and_then(convert::time),
        timeout_until: m.communication_disabled_until.and_then(convert::time),
    });
    Some(Person { account, member })
}

fn user_option(
    options: &[CommandDataOption],
    name: &str,
    resolved: Option<&InteractionDataResolved>,
) -> Option<Person> {
    match option(options, name)? {
        CommandOptionValue::User(id) => person(resolved, *id),
        _ => None,
    }
}

fn string_option(options: &[CommandDataOption], name: &str) -> Option<String> {
    match option(options, name)? {
        CommandOptionValue::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// An ATTACHMENT option, read from `resolved.attachments`.
fn attachment_option(
    options: &[CommandDataOption],
    name: &str,
    resolved: Option<&InteractionDataResolved>,
) -> Option<Upload> {
    match option(options, name)? {
        CommandOptionValue::Attachment(id) => upload(resolved?.attachments.get(id)?),
        _ => None,
    }
}

fn number_option(options: &[CommandDataOption], name: &str) -> Option<u64> {
    match option(options, name)? {
        CommandOptionValue::Integer(n) => u64::try_from(*n).ok().filter(|n| *n >= 1),
        _ => None,
    }
}

/// A rule option's value: a UUID picked from autocomplete, else the typed
/// text.
pub(super) fn rule_arg(value: &str) -> RuleArg {
    match uuid::Uuid::try_parse(value.trim()) {
        Ok(id) => RuleArg::Id(RuleId::from_uuid(id)),
        Err(_) => RuleArg::Name(value.trim().to_owned()),
    }
}

fn topic(value: &str) -> Option<Topic> {
    Some(match value {
        "cases" => Topic::Cases,
        "rules" => Topic::Rules,
        "appeals" => Topic::Appeals,
        "setup" => Topic::Setup,
        _ => return None,
    })
}

fn autocomplete(sub: &str, options: &[CommandDataOption]) -> Request {
    let command = match sub {
        "add" => AutocompleteFor::CaseAdd,
        "list" => AutocompleteFor::CaseList,
        _ => return UNKNOWN,
    };
    let focused = options.iter().find_map(|o| match &o.value {
        CommandOptionValue::Focused(query, _) if o.name == "rule" => Some(query.clone()),
        _ => None,
    });
    focused.map_or(UNKNOWN, |query| Request::Autocomplete { command, query })
}

/// `/case add`; a sent `file` that isn't resolved makes the request
/// unreadable, so a case is never added without the file it was sent with.
fn case_add(
    options: &[CommandDataOption],
    resolved: Option<&InteractionDataResolved>,
) -> Option<Request> {
    let file = match option(options, "file") {
        Some(_) => Some(attachment_option(options, "file", resolved)?),
        None => None,
    };
    Some(Request::CaseAdd {
        member: user_option(options, "member", resolved)?,
        rule: rule_arg(&string_option(options, "rule")?),
        private: match option(options, "private") {
            Some(CommandOptionValue::Boolean(b)) => Some(*b),
            _ => None,
        },
        file,
    })
}

/// `/case evidence`: a sent ATTACHMENT option that isn't resolved makes
/// the request unreadable rather than silently dropping the file.
fn case_evidence(
    options: &[CommandDataOption],
    resolved: Option<&InteractionDataResolved>,
) -> Option<Request> {
    let mut files = Vec::new();
    for name in ["file", "file2", "file3"] {
        if option(options, name).is_some() {
            files.push(attachment_option(options, name, resolved)?);
        }
    }
    Some(Request::CaseEvidence {
        number: number_option(options, "number")?,
        links: string_option(options, "link").map_or_else(Vec::new, |l| {
            l.split_whitespace().map(str::to_owned).collect()
        }),
        files,
        note: string_option(options, "note"),
    })
}

fn case(data: &CommandData, autocomplete_request: bool) -> Request {
    let Some((sub, options)) = data.options.first().and_then(|o| match &o.value {
        CommandOptionValue::SubCommand(options) => Some((o.name.as_str(), options.as_slice())),
        _ => None,
    }) else {
        return UNKNOWN;
    };
    if autocomplete_request {
        return autocomplete(sub, options);
    }
    let resolved = data.resolved.as_ref();
    let request = match sub {
        "add" => case_add(options, resolved),
        "evidence" => case_evidence(options, resolved),
        "view" => number_option(options, "number").map(|number| Request::CaseView { number }),
        "list" => Some(Request::CaseList {
            member: user_option(options, "member", resolved),
            rule: string_option(options, "rule").map(|r| rule_arg(&r)),
        }),
        "void" => number_option(options, "number")
            .zip(string_option(options, "reason"))
            .map(|(number, reason)| Request::CaseVoid { number, reason }),
        "history" => {
            user_option(options, "member", resolved).map(|member| Request::CaseHistory { member })
        }
        _ => None,
    };
    request.unwrap_or(UNKNOWN)
}

fn ops(data: &CommandData) -> Option<Request> {
    let first = data.options.first()?;
    if let CommandOptionValue::SubCommandGroup(subs) = &first.value {
        let sub = subs.first()?;
        return Some(Request::Ops(
            match (first.name.as_str(), sub.name.as_str()) {
                ("metrics", "overview") => OpsRequest::MetricsOverview,
                ("metrics", "servers") => OpsRequest::MetricsServers,
                ("metrics", "jobs") => OpsRequest::MetricsJobs,
                _ => return None,
            },
        ));
    }
    let CommandOptionValue::SubCommand(options) = &first.value else {
        return None;
    };
    Some(Request::Ops(match first.name.as_str() {
        "status" => OpsRequest::Status,
        "server" => OpsRequest::Server(string_option(options, "id")?),
        "jobs" => OpsRequest::Jobs,
        _ => return None,
    }))
}

fn target_message(data: &CommandData) -> Option<TargetMessage> {
    let id = data.target_id?.cast();
    let message = data.resolved.as_ref()?.messages.get(&id)?;
    Some(TargetMessage {
        id: convert::id(message.id)?,
        channel_id: convert::id(message.channel_id)?,
        author: convert::user(&message.author),
    })
}

fn target_member(data: &CommandData) -> Option<Request> {
    let id = data.target_id?.cast();
    person(data.resolved.as_ref(), id).map(|member| Request::AddCaseForMember { member })
}

/// Decodes command data; `autocomplete` is true for type 4 interactions.
pub(super) fn decode(data: &CommandData, autocomplete: bool) -> Request {
    let request = match (data.kind, data.name.as_str()) {
        (CommandType::ChatInput, "case") => Some(case(data, autocomplete)),
        (_, _) if autocomplete => None,
        (CommandType::ChatInput, "appeals") => Some(Request::Appeals),
        (CommandType::ChatInput, "rules") => Some(Request::Rules),
        (CommandType::ChatInput, "setup") => Some(Request::Setup),
        (CommandType::ChatInput, "ops") => ops(data),
        (CommandType::ChatInput, "help") => Some(Request::Help {
            topic: string_option(&data.options, "topic").and_then(|t| topic(&t)),
        }),
        (CommandType::Message, quack_text::command::ADD_CASE) => {
            target_message(data).map(|message| Request::AddCaseFromMessage { message })
        }
        (CommandType::Message, quack_text::command::ADD_AS_EVIDENCE) => {
            target_message(data).map(|message| Request::AddAsEvidence { message })
        }
        (CommandType::User, quack_text::command::ADD_CASE) => target_member(data),
        _ => None,
    };
    request.unwrap_or(UNKNOWN)
}
