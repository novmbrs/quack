//! §12.10 log channel entries: every entry kind and result, as a log
//! channel post and as the dashboard's text.

use quack_core::action::ActionKind;
use quack_core::modlog::{ActionDoneDetail, ModLogEntry, TicketClosedDetail};

use super::{Out, Sample, covering, sample};
use crate::log::{self, LogFacts};
use crate::model::Actor;
use crate::tests::fixtures::*;

pub fn facts() -> LogFacts {
    LogFacts {
        member: Some(sam()),
        case_number: Some(12),
        case_url: Some(URL.to_owned()),
        rule: Some("Spam".to_owned()),
        void_reason: Some("Wrong member.".to_owned()),
        timeout_ends_at: Some(later()),
        thread: Some(thread()),
        dashboard_url: Some("https://quack.example/guilds/111/mod-log".to_owned()),
        retry_action: None,
        ..LogFacts::new(t0(), ana())
    }
}

const SETTING_IDS: &[&str] = &[
    "setting.log_channel",
    "setting.appeal_channel",
    "setting.evidence_channel",
    "setting.appeal_invite",
    "setting.dm_intro",
    "setting.dm_footer",
    "setting.moderator_roles",
    "setting.rules_manager_roles",
    "setting.starter_notice",
    "setting.ticket_entry_channel",
    "setting.ticket_queue_channel",
    "setting.transcript_retention",
    "setting.honeypot_channel",
    "setting.honeypot_rule",
    "setting.honeypot_warning",
    "setting.activity_routes",
    "setting.activity_content",
    "module.tickets",
    "module.activity_log",
    "module.honeypot",
];

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for (id, name, e) in super::log_entries::entries() {
        let covers: &'static [&'static str] = if matches!(
            e,
            ModLogEntry::SettingsChanged(_)
                | ModLogEntry::ModuleSettingsChanged(_)
                | ModLogEntry::ModuleEnabled(_)
        ) {
            SETTING_IDS
        } else {
            &[]
        };
        let mut f = facts();
        if let ModLogEntry::ActionFailed(_) = e {
            f.retry_action = Some(action_id());
        }
        if let ModLogEntry::TicketClosed(_) = e {
            f.actor = Actor::Person(sam());
        }
        match e {
            ModLogEntry::HoneypotCaseFailed(_) => f.action = Some(ActionKind::Timeout),
            ModLogEntry::CaseCreateRefused(_) => f.action = Some(ActionKind::Kick),
            _ => {}
        }
        v.push(covering(
            id,
            covers,
            format!("{name} · post"),
            Out::Msg(log::post(&e, &f)),
        ));
        let plain = LogFacts {
            case_url: None,
            ..facts()
        };
        v.push(sample(
            id,
            format!("{name} · dashboard text"),
            Out::Text(log::text(&e, &plain)),
        ));
    }
    let bare = LogFacts::new(t0(), Actor::Quack);
    for (id, name, e) in super::log_entries::entries().into_iter().step_by(7) {
        v.push(sample(
            id,
            format!("{name} · no names known"),
            Out::Text(log::text(&e, &bare)),
        ));
    }
    v.push(sample(
        "log.action_done",
        "timeout, no change, end unknown · dashboard text",
        Out::Text(log::text(
            &ModLogEntry::ActionDone(ActionDoneDetail {
                case_number: 12,
                kind: ActionKind::Timeout,
                attempts: 1,
                no_change: true,
            }),
            &LogFacts {
                case_url: None,
                timeout_ends_at: None,
                ..facts()
            },
        )),
    ));
    v.push(sample(
        "log.ticket_closed",
        "closed by staff · post",
        Out::Msg(log::post(
            &ModLogEntry::TicketClosed(TicketClosedDetail {
                transcript_complete: true,
            }),
            &facts(),
        )),
    ));
    v
}
