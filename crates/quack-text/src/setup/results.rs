//! `/setup` result lines and the problems that stop a change (§12.11).

use quack_core::modules::{ModuleName, ModuleProblem};
use quack_core::permissions::{ChannelProblem, Permission};
use quack_core::settings::SettingsProblem;

use super::module_name;
use crate::action::permission;
use crate::fmt;
use crate::model::{ChannelRef, Person, RoleRef, Text};

/// What a `/setup` change did, shown at the top of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupResult {
    /// `setup.log.saved`.
    LogSaved(ChannelRef),
    /// `setup.log.cleared`.
    LogCleared,
    /// `setup.appeals.saved`.
    AppealsSaved(ChannelRef),
    /// `setup.appeals.cleared`.
    AppealsCleared,
    /// `setup.appeals.invite_saved`, with the stored invite link.
    InviteSaved(String),
    /// `setup.appeals.invite_cleared`.
    InviteCleared,
    /// `setup.evidence.saved`.
    EvidenceSaved(ChannelRef),
    /// `setup.evidence.cleared`.
    EvidenceCleared,
    /// `setup.tickets.entry_saved`.
    TicketEntrySaved(ChannelRef),
    /// `setup.tickets.queue_saved`.
    TicketQueueSaved(ChannelRef),
    /// `setup.tickets.on`, with the entry channel.
    TicketsOn(ChannelRef),
    /// `setup.tickets.off`.
    TicketsOff,
    /// `setup.honeypot.channel_saved`.
    HoneypotChannelSaved(ChannelRef),
    /// `setup.honeypot.rule_saved`, with the rule's name.
    HoneypotRuleSaved(String),
    /// `setup.honeypot.rule_cleared`.
    HoneypotRuleCleared,
    /// `setup.honeypot.rule_created`, with the rule's name.
    HoneypotRuleCreated(String),
    /// `setup.honeypot.warning_saved`.
    HoneypotWarningSaved,
    /// `setup.honeypot.on`, with the honeypot channel.
    HoneypotOn(ChannelRef),
    /// `setup.honeypot.off`.
    HoneypotOff,
    /// `setup.activity_log.saved`.
    ActivitySaved(ChannelRef),
    /// `setup.activity_log.on`.
    ActivityOn,
    /// `setup.activity_log.off`.
    ActivityOff,
    /// `setup.created`: a new staff-only channel.
    Created(ChannelRef),
    /// `setup.created_public`: a new channel members can see.
    CreatedPublic(ChannelRef),
    /// `setup.adopted`: a channel Quack created a moment ago and found.
    Adopted(ChannelRef),
}

/// The result line for a change.
pub fn result(r: &SetupResult) -> Text {
    let t = Text::new();
    match r {
        SetupResult::LogSaved(c) => t
            .s("Quack will post moderation log entries in ")
            .channel(c)
            .s("."),
        SetupResult::LogCleared => t.s("Quack stopped posting moderation log entries."),
        SetupResult::AppealsSaved(c) => t
            .s("Appeals will be posted in ")
            .channel(c)
            .s(", including open appeals that weren't posted yet."),
        SetupResult::AppealsCleared => t
            .s("Quack stopped posting new appeals. Review them with ")
            .command(crate::model::Command::Appeals)
            .s(" or on the dashboard."),
        SetupResult::InviteSaved(invite) => t
            .s("Members whose appeal is accepted will get this invite: ")
            .link(invite, invite),
        SetupResult::InviteCleared => t.s("The rejoin invite is removed."),
        SetupResult::EvidenceSaved(c) => {
            t.s("Quack will copy evidence files to ").channel(c).s(".")
        }
        SetupResult::EvidenceCleared => {
            t.s("Quack stopped copying evidence files. Existing copies stay where they are.")
        }
        SetupResult::TicketEntrySaved(c) => {
            t.s("Members will open tickets from ").channel(c).s(".")
        }
        SetupResult::TicketQueueSaved(c) => t.s("New tickets will be posted in ").channel(c).s("."),
        SetupResult::TicketsOn(c) => t
            .s("Tickets are on. The Open a ticket button is in ")
            .channel(c)
            .s("."),
        SetupResult::TicketsOff => {
            t.s("Tickets are off. Open tickets stay open until someone closes them.")
        }
        SetupResult::HoneypotChannelSaved(c) => t.s("The honeypot channel is ").channel(c).s("."),
        SetupResult::HoneypotRuleSaved(rule) => t
            .s("Posts in the honeypot channel will add a case under ")
            .bold_user(rule)
            .s("."),
        SetupResult::HoneypotRuleCleared => {
            t.s("The honeypot no longer uses a rule. Choose one before turning it on.")
        }
        SetupResult::HoneypotRuleCreated(rule) => t
            .s("Quack created the rule ")
            .bold_user(rule)
            .s(" for the honeypot. Change it on the dashboard if you want a different outcome."),
        SetupResult::HoneypotWarningSaved => t.s("The honeypot warning is updated."),
        SetupResult::HoneypotOn(c) => t.s("The honeypot is on in ").channel(c).s("."),
        SetupResult::HoneypotOff => t.s("The honeypot is off. Posts there no longer add cases."),
        SetupResult::ActivitySaved(c) => {
            t.s("Server activity will be posted in ").channel(c).s(".")
        }
        SetupResult::ActivityOn => t.s("The server activity log is on."),
        SetupResult::ActivityOff => t.s("The server activity log is off."),
        SetupResult::Created(c) => t
            .s("Quack created ")
            .channel(c)
            .s(" with staff-only permissions and saved it."),
        SetupResult::CreatedPublic(c) => t
            .s("Quack created ")
            .channel(c)
            .s(" so members can see it, and saved it."),
        SetupResult::Adopted(c) => t
            .s("Quack found ")
            .channel(c)
            .s(", which it created a moment ago, and saved it."),
    }
}

fn permissions_list(perms: &[Permission]) -> String {
    let names: Vec<String> = perms.iter().map(|p| permission(*p).to_owned()).collect();
    fmt::list(&names, "and")
}

/// `setup.create_failed.permission`. `name` is the channel name without `#`.
pub fn create_failed_permission(name: &str, perms: &[Permission]) -> Text {
    let n = fmt::one_line(name);
    Text::fixed(format!(
        "Quack couldn't create #{n} because it needs the {} permission",
        permissions_list(perms)
    ))
    .s(if perms.len() > 1 { "s" } else { "" })
    .s(". Give it to Quack's role, or choose an existing channel instead.")
}

/// `setup.create_failed.discord`.
pub fn create_failed_discord(name: &str) -> Text {
    Text::fixed(format!(
        "Discord didn't let Quack create #{}. Choose an existing channel instead.",
        fmt::one_line(name)
    ))
}

/// `setup.create_unconfirmed`.
pub fn create_unconfirmed(name: &str) -> Text {
    let n = fmt::one_line(name);
    Text::fixed(format!(
        "Quack couldn't confirm whether Discord created #{n}. Open the channel list: choose #{n} if it's there, or try again."
    ))
}

/// `setup.problem.*` and `setup.quack_missing_channel_permissions`: one
/// reason a channel isn't a usable staff channel. `roles` and `members`
/// give names for the ids the problem names.
pub fn channel_problem(
    p: &ChannelProblem,
    channel: &ChannelRef,
    roles: &[RoleRef],
    members: &[Person],
) -> Text {
    match p {
        ChannelProblem::NotText => Text::fixed("It isn't a text channel in this server."),
        ChannelProblem::EveryoneCanView => {
            Text::fixed("@everyone can view it. Deny View Channel for @everyone.")
        }
        ChannelProblem::RoleCanView(id) => {
            let role = roles
                .iter()
                .find(|r| r.id == *id)
                .cloned()
                .unwrap_or(RoleRef {
                    id: *id,
                    name: None,
                });
            Text::fixed("The role ")
                .role(&role)
                .s(" can view it but isn't a staff role.")
        }
        ChannelProblem::MemberCanView(id) => {
            let person = members
                .iter()
                .find(|m| m.id == id.account())
                .cloned()
                .unwrap_or(Person::unnamed(id.account()));
            Text::new()
                .person(&person)
                .s(" can view it but isn't staff.")
        }
        ChannelProblem::QuackMissing(perms) => quack_missing_channel_permissions(perms, channel),
        ChannelProblem::Inaccessible => Text::fixed(
            "Discord doesn't let Quack see this channel, so Quack can't check it. Give Quack the View Channel permission there.",
        ),
        ChannelProblem::Missing => {
            Text::fixed("This channel doesn't exist anymore. Choose another channel.")
        }
    }
}

/// `setup.quack_missing_channel_permissions`.
pub fn quack_missing_channel_permissions(perms: &[Permission], channel: &ChannelRef) -> Text {
    Text::fixed(format!("Quack needs {} in ", permissions_list(perms)))
        .channel(channel)
        .s(". Update the channel's permissions, then choose it again.")
}

/// `setup.channel_not_staff_only`: the channel and its problems, one per
/// line.
pub fn channel_not_staff_only(channel: &ChannelRef, problems: &[Text]) -> Text {
    let mut t = Text::new()
        .channel(channel)
        .s(" isn't staff-only, so Quack didn't save it:");
    for p in problems {
        t = t.s("\n").then(p.clone());
    }
    t.s("\nFix its permissions, then choose it again.")
}

/// `module.problem.*`: one reason a module can't be turned on. `rule` is
/// the honeypot rule's name and `channel` the channel a channel problem
/// names, when known.
pub fn module_problem(p: &ModuleProblem, rule: Option<&str>, channel: Option<&ChannelRef>) -> Text {
    let ch = |t: Text, id| match channel {
        Some(c) => t.channel(c),
        None => t.channel(&ChannelRef { id, name: None }),
    };
    match p {
        ModuleProblem::TicketEntryMissing => {
            Text::fixed("Choose the channel where members open tickets.")
        }
        ModuleProblem::TicketQueueMissing => Text::fixed("Choose the staff channel for new tickets."),
        ModuleProblem::TicketSameChannel => {
            Text::fixed("The entry channel and the staff channel must be different.")
        }
        ModuleProblem::HoneypotChannelMissing => Text::fixed("Choose the honeypot channel."),
        ModuleProblem::HoneypotRuleMissing => Text::fixed("Choose the rule the honeypot applies."),
        ModuleProblem::HoneypotRuleRequiredFields(_) => match rule {
            Some(r) => Text::fixed("The rule ").bold_user(r),
            None => Text::fixed("The rule the honeypot uses"),
        }
        .s(" has required context fields, which the honeypot can't fill in. Choose another rule or make those fields optional."),
        ModuleProblem::ActivityNoRoute => Text::fixed("Choose a channel for server activity."),
        ModuleProblem::ActivityNeedsAuditLog => Text::fixed(
            "Quack needs the View Audit Log permission to show who banned or unbanned a member.",
        ),
        ModuleProblem::ChannelPermissions { channel: id, missing } => {
            ch(Text::fixed(format!("Quack needs {} in ", permissions_list(missing))), *id).s(".")
        }
        ModuleProblem::ChannelNotStaffOnly(id) => ch(Text::new(), *id).s(" isn't staff-only."),
    }
}

/// `setup.module.not_ready`: the module and its problems, one per line.
pub fn module_not_ready(module: ModuleName, problems: &[Text]) -> Text {
    let mut t = Text::fixed(format!(
        "{} can't be turned on yet:",
        fmt::upper_first(module_name(module))
    ));
    for p in problems {
        t = t.s("\n").then(p.clone());
    }
    t
}

/// `setup.invite.invalid`.
pub fn invite_invalid() -> Text {
    Text::fixed("That isn't a Discord invite link. Use a link like https://discord.gg/abc123.")
}

/// `setting.problem.*`: one problem with a settings value. `max` is the
/// setting's character limit, for `too_long`.
pub fn settings_problem(p: SettingsProblem, max: usize) -> Text {
    match p {
        SettingsProblem::TooManyRoles => Text::fixed("Choose at most 25 roles."),
        SettingsProblem::DuplicateRole => Text::fixed("A role is listed more than once."),
        SettingsProblem::EveryoneRole => Text::fixed("@everyone can't be a staff role."),
        SettingsProblem::UnknownRole => {
            Text::fixed("One of these roles isn't in the server anymore. Choose a current role.")
        }
        SettingsProblem::InviteUrl => invite_invalid(),
        SettingsProblem::TooLong => Text::fixed(format!(
            "Use at most {} characters.",
            fmt::count(u64::try_from(max).unwrap_or(u64::MAX))
        )),
        SettingsProblem::RetentionDays => Text::fixed("Keep transcripts for 1 to 365 days."),
    }
}
