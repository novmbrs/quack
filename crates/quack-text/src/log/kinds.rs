//! The lead sentence of each moderation log entry (§12.10 `log.*`).

use quack_core::action::ActionKind;
use quack_core::case::VoidCause;
use quack_core::modlog::ModLogEntry;
use quack_core::modules::{
    ChannelSetting, HoneypotRefusal, LogChannelFailure, ModuleDisabledReason, ModuleName,
};
use quack_core::permissions::{DiscordPermission, MemberRefusal, MemberRefusalCode};

use super::LogFacts;
use crate::action;
use crate::error::{Voice, refusal_in};
use crate::fmt;
use crate::model::{Actor, Command, Text};
use crate::setup::module_name;

/// The quote under the lead sentence: the void reason of a staff void.
pub fn quote(entry: &ModLogEntry, f: &LogFacts) -> Option<Text> {
    match entry {
        ModLogEntry::CaseVoided(d) if d.cause == VoidCause::Staff => {
            f.void_reason.as_ref().map(|r| Text::new().user(r))
        }
        _ => None,
    }
}

/// `setting` in `log.channel_setting_cleared`.
const fn channel_setting(s: ChannelSetting) -> &'static str {
    match s {
        ChannelSetting::LogChannel => "log channel",
        ChannelSetting::AppealChannel => "appeal channel",
        ChannelSetting::EvidenceChannel => "evidence channel",
        ChannelSetting::TicketEntryChannel => "ticket entry channel",
        ChannelSetting::TicketQueueChannel => "ticket staff channel",
        ChannelSetting::HoneypotChannel => "honeypot channel",
        ChannelSetting::ActivityLogRoute => "server activity channel",
    }
}

fn refusal_from_code(code: MemberRefusalCode, permission: DiscordPermission) -> MemberRefusal {
    match code {
        MemberRefusalCode::QuackNotInServer => MemberRefusal::QuackNotInServer,
        MemberRefusalCode::ActorNotInServer => MemberRefusal::ActorNotInServer,
        MemberRefusalCode::MemberNotInServer => MemberRefusal::MemberNotInServer,
        MemberRefusalCode::MemberIsSelf => MemberRefusal::MemberIsSelf,
        MemberRefusalCode::MemberIsBot => MemberRefusal::MemberIsBot,
        MemberRefusalCode::MemberIsOwner => MemberRefusal::MemberIsOwner,
        MemberRefusalCode::MemberRoleTooHigh => MemberRefusal::MemberRoleTooHigh,
        MemberRefusalCode::QuackRoleTooLow => MemberRefusal::QuackRoleTooLow,
        MemberRefusalCode::QuackMissingPermission => {
            MemberRefusal::QuackMissingPermission(permission)
        }
        MemberRefusalCode::MemberIsAdministrator => MemberRefusal::MemberIsAdministrator,
    }
}

/// The lead sentence of an entry.
#[allow(
    clippy::too_many_lines,
    reason = "one arm per entry kind, each a catalog row"
)]
pub fn lead(entry: &ModLogEntry, f: &LogFacts) -> Text {
    let actor = || Text::new().actor(&f.actor);
    let member = |t: Text| f.member(t);
    let case = |t: Text, n: u64| f.case(t, n);
    // The permission a refusal names: the recorded one, else the level's
    // action's, else Ban Members (§12.10).
    let needed = |recorded: Option<DiscordPermission>| {
        recorded
            .or(f.action.map(ActionKind::permission))
            .unwrap_or(DiscordPermission::BanMembers)
    };
    match entry {
        ModLogEntry::CaseCreated(d) => super::actions::case_created(d, f),
        ModLogEntry::CaseCreateRefused(d) => f
            .rule(member(Text::fixed("No case was added for ")).s(" under "))
            .s(".")
            .sentence(refusal_in(
                refusal_from_code(d.refusal, needed(d.permission)),
                f.member.as_ref(),
                f.action,
                Voice::Log(&f.actor),
            )),
        ModLogEntry::CaseVoided(d) => match d.cause {
            VoidCause::Staff => {
                member(case(actor().s(" voided "), d.case_number).s(" for ")).s(".")
            }
            VoidCause::AppealAccepted => case(
                member(actor().s(" accepted ")).s("'s appeal, so "),
                d.case_number,
            )
            .s(" is voided."),
        },
        ModLogEntry::CaseContextEdited(d) => {
            case(actor().s(" edited the case context of "), d.case_number).s(".")
        }
        ModLogEntry::EvidenceAdded(d) => case(
            actor().s(format!(
                " added {} to ",
                fmt::plural(u64::from(d.count), "evidence item", "evidence items")
            )),
            d.case_number,
        )
        .s("."),
        ModLogEntry::ActionDone(d) => super::actions::action_done(d, f),
        ModLogEntry::ActionFailed(d) => super::actions::action_failed(d, f),
        ModLogEntry::ActionRetryRequested(d) => case(
            actor().s(format!(
                " chose to retry {} for ",
                action::retry_noun(d.kind)
            )),
            d.case_number,
        )
        .s("."),
        ModLogEntry::ActionDismissed(d) => case(
            actor().s(format!(" dismissed {} for ", action::failure_noun(d.kind))),
            d.case_number,
        )
        .s("."),
        ModLogEntry::ActionUndoRequested(d) => {
            let ban = matches!(d.kind, ActionKind::Unban | ActionKind::Ban);
            match (&f.actor, ban) {
                (Actor::Quack, true) => case(
                    member(Text::fixed("Quack will check whether it can unban ")).s(" for "),
                    d.case_number,
                ),
                (Actor::Quack, false) => case(
                    member(Text::fixed("Quack will check whether it can remove "))
                        .s("'s timeout for "),
                    d.case_number,
                ),
                (Actor::Person(_), true) => case(
                    member(actor().s(" asked Quack to unban ")).s(" for "),
                    d.case_number,
                ),
                (Actor::Person(_), false) => case(
                    member(actor().s(" asked Quack to remove ")).s("'s timeout for "),
                    d.case_number,
                ),
            }
            .s(".")
        }
        ModLogEntry::ActionUndoSkipped(d) => super::actions::undo_skipped(d, f),
        ModLogEntry::DmSent(d) => super::actions::dm_sent(*d, f),
        ModLogEntry::DmFailed(d) => super::actions::dm_failed(d, f),
        ModLogEntry::AppealSubmitted(d) => {
            case(member(Text::new()).s(" appealed "), d.case_number).s(".")
        }
        ModLogEntry::AppealInformationRequested(d) => case(
            member(actor().s(" asked ")).s(" for more information about their appeal for "),
            d.case_number,
        )
        .s("."),
        ModLogEntry::AppealInformationProvided(d) => case(
            member(Text::new()).s(" replied to the appeal for "),
            d.case_number,
        )
        .s("."),
        ModLogEntry::AppealAccepted(d) => case(
            member(actor().s(" accepted ")).s("'s appeal for "),
            d.case_number,
        )
        .s("."),
        ModLogEntry::AppealRejected(d) => case(
            member(actor().s(" rejected ")).s("'s appeal for "),
            d.case_number,
        )
        .s("."),
        ModLogEntry::AppealClosed(d) if d.because_case_voided => case(
            member(Text::fixed("Quack closed ")).s("'s appeal for "),
            d.case_number,
        )
        .s(" because the case was voided."),
        ModLogEntry::AppealClosed(d) => case(
            member(actor().s(" closed ")).s("'s appeal for "),
            d.case_number,
        )
        .s("."),
        ModLogEntry::AppealReopened(d) => case(
            member(actor().s(" reopened ")).s("'s appeal for "),
            d.case_number,
        )
        .s(" and asked for more information."),
        ModLogEntry::RuleCreated(d) => actor().s(" created the rule ").bold_user(&d.name).s("."),
        ModLogEntry::RuleEdited(d) => actor()
            .s(" edited the rule ")
            .bold_user(&d.name)
            .s(format!(
                ". New cases use the updated rule (version {}).",
                d.version
            )),
        ModLogEntry::RuleArchived(d) => actor()
            .s(" archived the rule ")
            .bold_user(&d.name)
            .s(". It can't be applied until someone restores it."),
        ModLogEntry::RuleRestored(d) => actor().s(" restored the rule ").bold_user(&d.name).s("."),
        ModLogEntry::RuleImported(d) => actor().s(" imported the rule ").bold_user(&d.name).s("."),
        ModLogEntry::SettingsChanged(d) => {
            let names = d.fields.iter().map(|k| super::setting_name(k));
            changed(actor().s(" changed Quack's settings"), names)
        }
        ModLogEntry::ModuleEnabled(d) => {
            actor().s(format!(" turned on {}.", module_name(d.module)))
        }
        ModLogEntry::ModuleDisabled(d) => match d.reason {
            None => actor().s(format!(" turned off {}.", module_name(d.module))),
            Some(ModuleDisabledReason::ChannelDeleted) => Text::fixed(format!(
                "Quack turned off {} because its channel was deleted. Choose a new channel in ",
                module_name(d.module)
            ))
            .command(Command::Setup)
            .s(", then turn it on."),
            Some(ModuleDisabledReason::QuackLeft) => Text::fixed(format!(
                "Quack turned off {} because Quack left the server.",
                module_name(d.module)
            )),
        },
        ModLogEntry::ModuleSettingsChanged(d) => {
            let names = d
                .fields
                .iter()
                .map(|k| super::module_setting_name(d.module, k));
            let module = match d.module {
                ModuleName::Tickets => "ticket",
                ModuleName::ActivityLog => "server activity log",
                ModuleName::Honeypot => "honeypot",
            };
            changed(actor().s(format!(" changed the {module} settings")), names)
        }
        ModLogEntry::QuackJoined(d) if d.first_join => f
            .rule(Text::fixed(
                "Quack joined the server and created the starter rule ",
            ))
            .s(". Review it on the dashboard."),
        ModLogEntry::QuackJoined(_) => Text::fixed("Quack rejoined the server."),
        ModLogEntry::QuackLeft(_) => Text::fixed("Quack left the server."),
        ModLogEntry::ChannelSettingCleared(d) => Text::fixed(format!(
            "The {} was deleted. Choose a new channel in ",
            channel_setting(d.setting)
        ))
        .command(Command::Setup)
        .s("."),
        ModLogEntry::LogChannelFailed(d) => match d.reason {
            LogChannelFailure::NotStaffOnly => Text::fixed(
                "Quack stopped posting to the log channel because members outside staff can see it. The entries are saved in the moderation log. Fix the channel's View Channel permissions or choose another log channel in ",
            )
            .command(Command::Setup)
            .s("."),
            LogChannelFailure::DiscordRefused => Text::fixed(
                "Discord didn't let Quack post in the log channel. The entries are saved in the moderation log. In Edit Channel → Permissions, check View Channel and Send Messages for Quack.",
            ),
            LogChannelFailure::RetriesExhausted => Text::fixed(
                "Quack couldn't confirm whether an entry reached the log channel. The entries are saved in the moderation log.",
            ),
        },
        ModLogEntry::TicketOpened(_) => {
            let t = member(Text::new()).s(" opened a ticket");
            match &f.thread {
                Some(thread) => t.s(": ").channel(thread).s("."),
                None => t.s("."),
            }
        }
        ModLogEntry::TicketClosed(d) => {
            let by_member =
                matches!((&f.actor, &f.member), (Actor::Person(a), Some(m)) if a.id == m.id);
            let t = if by_member {
                member(Text::new()).s(" closed their ticket.")
            } else {
                member(actor().s(" closed ")).s("'s ticket.")
            };
            let t = t.s(" Open the ticket on the dashboard to check for a transcript.");
            if d.transcript_complete {
                t
            } else {
                t.s(" Some messages may be missing from it.")
            }
        }
        ModLogEntry::HoneypotCaseFailed(d) => {
            let review = || {
                Text::fixed("Review the post and use ")
                    .command(Command::CaseAdd)
                    .s(" if a case is needed.")
            };
            let why = match d.refusal {
                HoneypotRefusal::Member(code) => refusal_in(
                    refusal_from_code(code, needed(None)),
                    f.member.as_ref(),
                    f.action,
                    Voice::Log(&Actor::Quack),
                ),
                HoneypotRefusal::RuleUnavailable => Text::fixed(
                    "The honeypot rule is archived or no longer exists. Choose a rule in ",
                )
                .command(Command::Setup)
                .s(" or on the dashboard."),
                HoneypotRefusal::PolicyChanged => {
                    Text::fixed("The honeypot settings changed while Quack was adding it.")
                        .sentence(review())
                }
                HoneypotRefusal::DiscordUnavailable => {
                    Text::fixed("Quack couldn't reach Discord in time.").sentence(review())
                }
            };
            member(Text::fixed("Quack didn't add a honeypot case for "))
                .s(".")
                .sentence(why)
        }
        ModLogEntry::Legacy(d) => match legacy_action(&d.action) {
            name if name.is_empty() => {
                Text::fixed("This entry was imported from an earlier version of Quack.")
            }
            name => Text::fixed("Imported from an earlier version of Quack: ")
                .user(name)
                .s("."),
        },
    }
}

/// "… changed Quack's settings: the log channel." The names Quack can't
/// name are left out; with none left, the sentence ends at "settings".
fn changed(lead: Text, names: impl Iterator<Item = &'static str>) -> Text {
    let known: Vec<&'static str> = names.filter(|n| *n != "another setting").collect();
    if known.is_empty() {
        lead.s(". Open the settings on the dashboard to review them.")
    } else {
        lead.s(format!(": {}.", super::fields(known)))
    }
}

/// A Go-era action name in today's words: separators become spaces and the
/// old vocabulary becomes the glossary's ("template" is "rule",
/// "templates" is "rules"; "audit log" and "audit" are "moderation log").
pub(crate) fn legacy_action(name: &str) -> String {
    let words: Vec<String> = name
        .split(['_', '.', ':', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let next = words.get(i + 1).map(String::as_str);
        if w == "audit" && matches!(next, Some("log" | "logs")) {
            out.push(if next == Some("logs") {
                "moderation logs"
            } else {
                "moderation log"
            });
            i += 2;
            continue;
        }
        out.push(match w {
            "template" | "policy" => "rule",
            "templates" | "policies" => "rules",
            "execution" | "punishment" | "enforcement" => "action",
            "executions" | "punishments" | "enforcements" => "actions",
            "infraction" | "incident" => "case",
            "infractions" | "incidents" => "cases",
            "notification" | "notice" => "DM",
            "notifications" | "notices" => "DMs",
            "audit" => "moderation log",
            "mirror" => "log channel",
            "mirrors" => "log channels",
            "user" | "target" | "offender" => "member",
            "users" | "targets" | "offenders" => "members",
            "mod" => "moderator",
            "mods" => "moderators",
            "admin" => "administrator",
            "admins" => "administrators",
            _ => w,
        });
        i += 1;
    }
    out.join(" ")
}
