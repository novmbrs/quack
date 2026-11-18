//! `/setup` result lines and the problems that stop a change (§12.11).

use quack_core::modules::{ModuleName, ModuleProblem};
use quack_core::permissions::{ChannelProblem, Permission};
use quack_core::settings::SettingsProblem;

use super::module_name;
use crate::access::{self, Reader};
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
        SetupResult::LogCleared => t.s(
            "Quack stopped posting entries to Discord. You can still read them in the moderation log on the dashboard.",
        ),
        SetupResult::AppealsSaved(c) => t
            .s("Appeals will be posted in ")
            .channel(c)
            .s(", including open appeals that weren't posted yet."),
        SetupResult::AppealsCleared => t
            .s("Quack stopped posting new appeals to Discord. You can still review them with ")
            .command(crate::model::Command::Appeals)
            .s(" or on the dashboard."),
        SetupResult::InviteSaved(invite) => t
            .s("Rejoin invite saved: ")
            .link(invite, invite)
            .s(". Quack includes it in DMs about accepted appeals for kicks and bans."),
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
            .s("Tickets are on. Quack will post the Open a ticket button in ")
            .channel(c)
            .s("."),
        SetupResult::TicketsOff => {
            t.s("Tickets are off. Open tickets stay open until someone closes them.")
        }
        SetupResult::HoneypotChannelSaved(c) => t.s("The honeypot channel is ").channel(c).s("."),
        SetupResult::HoneypotRuleSaved(rule) => {
            t.s("The honeypot rule is now ").bold_user(rule).s(".")
        }
        SetupResult::HoneypotRuleCleared => {
            t.s("The honeypot no longer uses a rule. Choose one before turning it on.")
        }
        SetupResult::HoneypotRuleCreated(rule) => t
            .s("Quack created the rule ")
            .bold_user(rule)
            .s(" for the honeypot. Edit it on the dashboard to change what it does."),
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
        SetupResult::Adopted(c) => t.s("Quack found ").channel(c).s(" and saved it."),
    }
}

/// `setup.create_failed.permission`. `name` is the channel name without `#`.
pub fn create_failed_permission(name: &str, perms: &[Permission]) -> Text {
    let n = fmt::one_line(name);
    Text::fixed(format!("Quack couldn't create #{n}."))
        .sentence(access::quack_role_needs(perms, Reader::Administrator))
        .s(" Or choose an existing channel.")
}

/// `setup.create_failed.discord`.
pub fn create_failed_discord(name: &str) -> Text {
    Text::fixed(format!(
        "Discord didn't let Quack create #{}. Choose an existing channel.",
        fmt::one_line(name)
    ))
}

/// `setup.create_unconfirmed`.
pub fn create_unconfirmed(name: &str) -> Text {
    let n = fmt::one_line(name);
    Text::fixed(format!(
        "Quack couldn't confirm whether Discord created #{n}. Check the channel list. If #{n} is there, choose it here. Otherwise, create it again."
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
        ChannelProblem::NotText => Text::fixed("Choose a text channel in this server."),
        ChannelProblem::EveryoneCanView => Text::fixed(
            "Everyone can view this channel. In Edit Channel → Permissions, turn off View Channel for the @everyone role.",
        ),
        ChannelProblem::RoleCanView(id) => {
            let role = roles
                .iter()
                .find(|r| r.id == *id)
                .cloned()
                .unwrap_or(RoleRef {
                    id: *id,
                    name: None,
                });
            Text::new().role(&role).s(
                " can view this channel. In Edit Channel → Permissions, remove its View Channel access or choose another channel.",
            )
        }
        ChannelProblem::MemberCanView(id) => {
            let person = members
                .iter()
                .find(|m| m.id == id.account())
                .cloned()
                .unwrap_or(Person::unnamed(id.account()));
            Text::new().person(&person).s(
                " can view this channel. Remove their access in Edit Channel → Permissions or choose another channel.",
            )
        }
        ChannelProblem::QuackMissing(perms) => quack_missing_channel_permissions(perms, channel),
        ChannelProblem::Inaccessible => Text::fixed(
            "Quack can't check this channel. In Edit Channel → Permissions, allow View Channel for Quack, then choose the channel again.",
        ),
        ChannelProblem::Missing => {
            Text::fixed("This channel no longer exists. Choose another channel.")
        }
    }
}

/// `setup.quack_missing_channel_permissions`.
pub fn quack_missing_channel_permissions(perms: &[Permission], channel: &ChannelRef) -> Text {
    access::quack_channel_needs(channel, perms, Reader::Administrator)
        .s(" Then choose the channel again.")
}

/// `setup.channel_not_staff_only`: the channel and its problems, one per
/// line; each problem says how to fix it.
pub fn channel_not_staff_only(channel: &ChannelRef, problems: &[Text]) -> Text {
    let mut t = Text::fixed("Quack didn't save ").channel(channel).s(":");
    for p in problems {
        t = t.s("\n").then(p.clone());
    }
    t
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
        ModuleProblem::TicketEntryMissing => Text::fixed("Choose where members open tickets."),
        ModuleProblem::TicketQueueMissing => Text::fixed("Choose a staff channel for new tickets."),
        ModuleProblem::TicketSameChannel => {
            Text::fixed("The entry channel and the staff channel must be different.")
        }
        ModuleProblem::HoneypotChannelMissing => Text::fixed("Choose the honeypot channel."),
        ModuleProblem::HoneypotRuleMissing => Text::fixed("Choose the rule the honeypot applies."),
        ModuleProblem::HoneypotRuleRequiredFields(_) => match rule {
            Some(r) => Text::fixed("The rule ").bold_user(r),
            None => Text::fixed("The rule the honeypot uses"),
        }
        .s(" has required case context, which the honeypot can't fill in. Choose another rule or make those fields optional on the dashboard."),
        ModuleProblem::ActivityNoRoute => Text::fixed("Choose a channel for server activity."),
        ModuleProblem::ActivityNeedsAuditLog => {
            access::quack_role_needs(&[Permission::ViewAuditLog], Reader::Administrator)
                .s(" Quack uses it to show who banned or unbanned a member.")
        }
        ModuleProblem::ChannelPermissions { channel: id, missing } => {
            let c = channel.cloned().unwrap_or(ChannelRef { id: *id, name: None });
            access::quack_channel_needs(&c, missing, Reader::Administrator)
        }
        ModuleProblem::ChannelNotStaffOnly(id) => ch(Text::new(), *id).s(
            " can be viewed by members outside staff. Fix its View Channel permissions or choose another channel.",
        ),
    }
}

/// `setup.module.not_ready`: the module and its problems, one per line.
pub fn module_not_ready(module: ModuleName, problems: &[Text]) -> Text {
    let name = fmt::upper_first(module_name(module));
    if problems.is_empty() {
        return Text::fixed(format!("{name} can't be turned on yet. Open "))
            .command(crate::model::Command::Setup)
            .s(" to see what needs fixing.");
    }
    let mut t = Text::fixed(format!("{name} can't be turned on yet:"));
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
        SettingsProblem::DuplicateRole => Text::fixed("Choose each role once."),
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
