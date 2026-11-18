//! §12.11 setup.

use quack_core::modules::{ModuleName, ModuleProblem};
use quack_core::permissions::{ChannelProblem, Permission};
use quack_core::settings::SettingsProblem;

use super::{Out, Sample, covering, label, modal, msg, sample, text};
use crate::actions::SetupArea;
use crate::case::PickerRule;
use crate::setup::{self, ActivityRoutes, ChannelKind, SetupResult, SetupState};
use crate::tests::fixtures::*;

fn state_on() -> SetupState {
    SetupState {
        log: Some(channel("moderation-log")),
        appeals: Some(channel("appeals")),
        invite_set: true,
        evidence: Some(channel("evidence")),
        tickets_on: true,
        ticket_entry: Some(channel("support")),
        ticket_queue: Some(channel("ticket-log")),
        honeypot_on: true,
        honeypot_channel: Some(channel("honeypot")),
        honeypot_rule: Some("Honeypot".to_owned()),
        activity_on: true,
        activity: ActivityRoutes::One(channel("server-log")),
        settings_url: "https://quack.example/guilds/111/settings".to_owned(),
    }
}

fn state_off() -> SetupState {
    SetupState {
        settings_url: "https://quack.example/guilds/111/settings".to_owned(),
        ..SetupState::default()
    }
}

const HOME_IDS: &[&str] = &[
    "setup.home.heading",
    "setup.home.intro",
    "setup.home.log",
    "setup.home.appeals",
    "setup.home.evidence",
    "setup.home.tickets",
    "setup.home.honeypot",
    "setup.home.activity_log",
    "setup.button.log",
    "setup.button.appeals",
    "setup.button.evidence",
    "setup.button.tickets",
    "setup.button.honeypot",
    "setup.button.activity_log",
];

const AREA_IDS: &[&str] = &[
    "setup.button.back",
    "setup.button.create",
    "setup.button.create_ticket_channels",
    "setup.button.turn_on",
    "setup.button.turn_off",
    "setup.button.stop_log",
    "setup.button.stop_evidence",
    "setup.button.set_invite",
    "setup.button.remove_invite",
    "setup.button.stop_appeals",
    "setup.button.clear_rule",
    "setup.button.edit_warning",
    "setup.select.channel",
    "setup.select.entry",
    "setup.select.queue",
    "setup.select.rule",
    "setup.log.explain",
    "setup.appeals.explain",
    "setup.evidence.explain",
    "setup.tickets.explain",
    "setup.honeypot.explain",
    "setup.activity_log.explain",
];

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "setup.home",
        HOME_IDS,
        "everything on",
        Out::Msg(setup::home(&state_on(), None)),
    ));
    v.push(sample(
        "setup.home",
        "nothing set, with a result",
        Out::Msg(setup::home(
            &state_off(),
            Some(setup::result(&SetupResult::LogCleared)),
        )),
    ));
    v.push(sample(
        "setup.home",
        "several activity channels",
        Out::Msg(setup::home(
            &SetupState {
                activity: ActivityRoutes::Several,
                ..state_on()
            },
            None,
        )),
    ));
    let rules = [PickerRule {
        id: rule_id(),
        name: "Honeypot".to_owned(),
        description: "Posting in the honeypot channel.".to_owned(),
    }];
    for area in [
        SetupArea::Log,
        SetupArea::Appeals,
        SetupArea::Evidence,
        SetupArea::Tickets,
        SetupArea::Honeypot,
        SetupArea::ActivityLog,
    ] {
        v.push(covering(
            "setup.area",
            AREA_IDS,
            format!("{area:?} set"),
            Out::Msg(setup::area(area, &state_on(), None, &rules)),
        ));
        v.push(sample(
            "setup.area",
            format!("{area:?} not set"),
            Out::Msg(setup::area(area, &state_off(), None, &[])),
        ));
    }
    let off_with_rule = SetupState {
        honeypot_on: false,
        ..state_on()
    };
    v.push(sample(
        "setup.area",
        "Honeypot off with a rule",
        Out::Msg(setup::area(
            SetupArea::Honeypot,
            &off_with_rule,
            None,
            &rules,
        )),
    ));
    let c = channel("staff");
    for (id, r) in [
        ("setup.log.saved", SetupResult::LogSaved(c.clone())),
        ("setup.log.cleared", SetupResult::LogCleared),
        ("setup.appeals.saved", SetupResult::AppealsSaved(c.clone())),
        ("setup.appeals.cleared", SetupResult::AppealsCleared),
        (
            "setup.appeals.invite_saved",
            SetupResult::InviteSaved("https://discord.gg/abc123".to_owned()),
        ),
        ("setup.appeals.invite_cleared", SetupResult::InviteCleared),
        (
            "setup.evidence.saved",
            SetupResult::EvidenceSaved(c.clone()),
        ),
        ("setup.evidence.cleared", SetupResult::EvidenceCleared),
        (
            "setup.tickets.entry_saved",
            SetupResult::TicketEntrySaved(c.clone()),
        ),
        (
            "setup.tickets.queue_saved",
            SetupResult::TicketQueueSaved(c.clone()),
        ),
        ("setup.tickets.on", SetupResult::TicketsOn(c.clone())),
        ("setup.tickets.off", SetupResult::TicketsOff),
        (
            "setup.honeypot.channel_saved",
            SetupResult::HoneypotChannelSaved(c.clone()),
        ),
        (
            "setup.honeypot.rule_saved",
            SetupResult::HoneypotRuleSaved("Honeypot".to_owned()),
        ),
        (
            "setup.honeypot.rule_cleared",
            SetupResult::HoneypotRuleCleared,
        ),
        (
            "setup.honeypot.rule_created",
            SetupResult::HoneypotRuleCreated("Honeypot".to_owned()),
        ),
        (
            "setup.honeypot.warning_saved",
            SetupResult::HoneypotWarningSaved,
        ),
        ("setup.honeypot.on", SetupResult::HoneypotOn(c.clone())),
        ("setup.honeypot.off", SetupResult::HoneypotOff),
        (
            "setup.activity_log.saved",
            SetupResult::ActivitySaved(c.clone()),
        ),
        ("setup.activity_log.on", SetupResult::ActivityOn),
        ("setup.activity_log.off", SetupResult::ActivityOff),
        ("setup.created", SetupResult::Created(c.clone())),
        (
            "setup.created_public",
            SetupResult::CreatedPublic(c.clone()),
        ),
        ("setup.adopted", SetupResult::Adopted(c.clone())),
    ] {
        v.push(text(id, "", setup::result(&r)));
    }
    v.push(text(
        "setup.create_failed.permission",
        "one permission",
        setup::create_failed_permission("moderation-log", &[Permission::ManageChannels]),
    ));
    v.push(text(
        "setup.create_failed.permission",
        "two permissions",
        setup::create_failed_permission(
            "support",
            &[Permission::ManageChannels, Permission::ManageThreads],
        ),
    ));
    v.push(text(
        "setup.create_failed.discord",
        "",
        setup::create_failed_discord("moderation-log"),
    ));
    v.push(text(
        "setup.create_unconfirmed",
        "",
        setup::create_unconfirmed("moderation-log"),
    ));
    let known_roles = [role()];
    let members = [sam()];
    let problems: Vec<ChannelProblem> = vec![
        ChannelProblem::NotText,
        ChannelProblem::EveryoneCanView,
        ChannelProblem::RoleCanView(role().id),
        ChannelProblem::MemberCanView(member_id()),
        ChannelProblem::QuackMissing(vec![
            Permission::ViewChannel,
            Permission::SendMessages,
            Permission::AttachFiles,
        ]),
        ChannelProblem::Inaccessible,
        ChannelProblem::Missing,
    ];
    let lines: Vec<_> = problems
        .iter()
        .map(|p| setup::channel_problem(p, &c, &known_roles, &members))
        .collect();
    for (p, line) in problems.iter().zip(&lines) {
        v.push(covering(
            "setup.problem",
            &[
                "setup.problem.not_text",
                "setup.problem.everyone_can_view",
                "setup.problem.role_can_view",
                "setup.problem.member_can_view",
                "setup.quack_missing_channel_permissions",
            ],
            p.code(),
            Out::Text(line.clone()),
        ));
    }
    v.push(text(
        "setup.problem",
        "role and member without names",
        setup::channel_problem(&ChannelProblem::RoleCanView(role().id), &c, &[], &[]),
    ));
    v.push(text(
        "setup.channel_not_staff_only",
        "",
        setup::channel_not_staff_only(&c, &lines[..4]),
    ));
    let module_problems = vec![
        ModuleProblem::TicketEntryMissing,
        ModuleProblem::TicketQueueMissing,
        ModuleProblem::TicketSameChannel,
        ModuleProblem::HoneypotChannelMissing,
        ModuleProblem::HoneypotRuleMissing,
        ModuleProblem::HoneypotRuleRequiredFields(rule_id()),
        ModuleProblem::ActivityNoRoute,
        ModuleProblem::ActivityNeedsAuditLog,
        ModuleProblem::ChannelPermissions {
            channel: c.id,
            missing: vec![Permission::ManageMessages, Permission::ReadMessageHistory],
        },
        ModuleProblem::ChannelNotStaffOnly(c.id),
    ];
    let module_lines: Vec<_> = module_problems
        .iter()
        .map(|p| setup::module_problem(p, Some("Honeypot"), Some(&c)))
        .collect();
    for (p, line) in module_problems.iter().zip(&module_lines) {
        v.push(covering(
            "module.problem",
            &[
                "module.problem.ticket_entry_missing",
                "module.problem.ticket_queue_missing",
                "module.problem.ticket_same_channel",
                "module.problem.honeypot_channel_missing",
                "module.problem.honeypot_rule_missing",
                "module.problem.honeypot_rule_required_fields",
                "module.problem.activity_no_route",
                "module.problem.activity_needs_audit_log",
                "module.problem.channel_permissions",
                "module.problem.channel_not_staff_only",
            ],
            p.code(),
            Out::Text(line.clone()),
        ));
    }
    for m in ModuleName::ALL {
        v.push(text(
            "setup.module.not_ready",
            format!("{m:?}"),
            setup::module_not_ready(*m, &module_lines[..2]),
        ));
    }
    v.push(covering(
        "setup.invite.modal",
        &[
            "setup.invite.modal.title",
            "setup.invite.modal.label",
            "setup.invite.modal.description",
        ],
        "",
        Out::Modal(setup::invite_modal(Some("https://discord.gg/abc123"))),
    ));
    v.push(text("setup.invite.invalid", "", setup::invite_invalid()));
    v.push(covering(
        "setup.warning.modal",
        &[
            "setup.warning.modal.title",
            "setup.warning.modal.label",
            "setup.warning.modal.description",
        ],
        "",
        Out::Modal(setup::warning_modal(None)),
    ));
    v.push(modal(
        "setup.warning.modal",
        "prefilled, maximal",
        setup::warning_modal(Some(&long(1_500))),
    ));
    for p in SettingsProblem::ALL {
        v.push(text(
            "setting.problem",
            format!("{p:?}"),
            setup::settings_problem(*p, 1_000),
        ));
    }
    for (id, kind) in [
        ("channel.log", ChannelKind::Log),
        ("channel.appeals", ChannelKind::Appeals),
        ("channel.evidence", ChannelKind::Evidence),
        ("channel.ticket_entry", ChannelKind::TicketEntry),
        ("channel.ticket_queue", ChannelKind::TicketQueue),
        ("channel.honeypot", ChannelKind::Honeypot),
        ("channel.activity_log", ChannelKind::ActivityLog),
    ] {
        v.push(label(
            id,
            setup::channel_name(kind),
            crate::budget::CHANNEL_NAME,
        ));
        v.push(sample(
            id,
            "topic",
            Out::Str(
                setup::channel_topic(kind).to_owned(),
                crate::budget::CHANNEL_TOPIC,
            ),
        ));
        if let Some(m) = setup::first_message(kind) {
            v.push(msg(id, "first message", m));
        }
    }
    v
}
