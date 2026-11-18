//! §12.13 tickets, server activity log, and honeypot.

use quack_core::modules::{TicketState, TranscriptPartial};
use quack_core::rule::LevelAction;

use super::{Out, Sample, covering, label, msg, sample, text};
use crate::honeypot::{self, Warning};
use crate::model::Actor;
use crate::tests::fixtures::*;
use crate::ticket::{self, TicketPost, TranscriptPlace};

fn ticket_post() -> TicketPost {
    TicketPost {
        ticket: ticket_id(),
        member: sam(),
        state: TicketState::Open,
        open_failed: false,
        thread: Some(thread()),
        thread_url: Some("https://discord.com/channels/111/555".to_owned()),
        opened_at: t0(),
        closed: None,
        transcript: TranscriptPlace::NotYet,
        partial: None,
        thread_not_deleted: false,
    }
}

const POST_IDS: &[&str] = &[
    "ticket.post.heading",
    "ticket.post.open",
    "ticket.post.opening",
    "ticket.post.open_failed",
    "ticket.post.closing",
    "ticket.post.closed",
    "ticket.post.transcript_attached",
    "ticket.post.transcript_dashboard",
    "ticket.post.transcript_expired",
    "ticket.post.partial",
    "ticket.post.partial.history",
    "ticket.post.partial.trimmed",
    "ticket.post.thread_not_deleted",
    "ticket.button.close",
    "ticket.button.open_thread",
    "ticket.button.fix_access",
];

fn tickets() -> Vec<Sample> {
    let mut v = Vec::new();
    v.push(covering(
        "ticket.panel",
        &[
            "ticket.panel.heading",
            "ticket.panel.text",
            "ticket.panel.button",
        ],
        "on",
        Out::Msg(ticket::panel(true)),
    ));
    v.push(covering(
        "ticket.panel",
        &["ticket.panel.off"],
        "off",
        Out::Msg(ticket::panel(false)),
    ));
    v.push(msg(
        "ticket.panel.moved",
        "",
        ticket::panel_moved(&channel("support")),
    ));
    v.push(text(
        "ticket.open.opening",
        "",
        ticket::open_opening(&channel("ticket-log")),
    ));
    v.push(text(
        "ticket.open.exists",
        "",
        ticket::open_exists(&thread()),
    ));
    v.push(text("ticket.open.closing", "", ticket::open_closing()));
    v.push(text("ticket.open.disabled", "", ticket::open_disabled()));
    v.push(text(
        "ticket.open.failed",
        "",
        ticket::open_failed(&channel("ticket-log")),
    ));
    v.push(msg(
        "ticket.welcome",
        "",
        ticket::welcome(&sam(), ticket_id()),
    ));
    let closed = Some((ana(), later()));
    for (name, p) in [
        (
            "opening",
            TicketPost {
                state: TicketState::Opening,
                thread: None,
                thread_url: None,
                ..ticket_post()
            },
        ),
        ("open", ticket_post()),
        (
            "closing",
            TicketPost {
                state: TicketState::Closing,
                ..ticket_post()
            },
        ),
        (
            "open failed",
            TicketPost {
                state: TicketState::Closed,
                open_failed: true,
                ..ticket_post()
            },
        ),
        (
            "closed, attached",
            TicketPost {
                state: TicketState::Closed,
                closed: closed.clone(),
                transcript: TranscriptPlace::Attached,
                ..ticket_post()
            },
        ),
        (
            "closed by Quack, on the dashboard, partial history",
            TicketPost {
                state: TicketState::Closed,
                closed: Some((Actor::Quack, later())),
                transcript: TranscriptPlace::Dashboard(later()),
                partial: Some(TranscriptPartial::History),
                ..ticket_post()
            },
        ),
        (
            "closed, expired, trimmed, thread kept",
            TicketPost {
                state: TicketState::Closed,
                closed,
                transcript: TranscriptPlace::Expired(later()),
                partial: Some(TranscriptPartial::Trimmed),
                thread_not_deleted: true,
                ..ticket_post()
            },
        ),
    ] {
        v.push(covering(
            "ticket.post",
            POST_IDS,
            name,
            Out::Msg(ticket::post(&p)),
        ));
    }
    v.push(text("ticket.close.closing", "", ticket::close_closing()));
    v.push(text("ticket.close.not_open", "", ticket::close_not_open()));
    v.push(text(
        "ticket.close.not_allowed",
        "",
        ticket::close_not_allowed(),
    ));
    for n in [0, 1, 3] {
        v.push(text(
            "ticket.fix.done",
            format!("removed {n}"),
            ticket::fix_done(&sam(), n),
        ));
    }
    v.push(text(
        "ticket.fix.failed",
        "",
        ticket::fix_failed(&channel("ticket-log")),
    ));
    v.push(covering(
        "dm.ticket_closed",
        &["dm.ticket_closed.heading", "dm.ticket_closed.text"],
        "",
        Out::Msg(ticket::closed_dm("Duck Pond", "transcript.txt")),
    ));
    v.push(label(
        "ticket.thread_name",
        ticket::thread_name(ticket_id()),
        crate::budget::CHANNEL_NAME,
    ));
    v.push(label(
        "ticket.transcript.header",
        ticket::transcript_header("Duck Pond", t0(), "Sam", 222, later()),
        2_000,
    ));
    v.push(label(
        "ticket.transcript.line",
        ticket::transcript_line(t0(), "Sam", 222, "Hello, I need help."),
        2_000,
    ));
    v.push(label(
        "ticket.transcript.file_line",
        ticket::transcript_file_line("screenshot.png", 123_456),
        300,
    ));
    for p in TranscriptPartial::ALL {
        v.push(sample(
            "ticket.transcript.partial",
            format!("{p:?}"),
            Out::Str(ticket::transcript_partial(*p), 300),
        ));
    }
    v
}

const WARNING_IDS: &[&str] = &[
    "honeypot.warning.heading",
    "honeypot.warning.one_outcome",
    "honeypot.warning.many_outcomes",
    "honeypot.warning.warning_only",
    "honeypot.warning.tail",
    "honeypot.warning.count",
    "honeypot.outcome.ban",
    "honeypot.outcome.kick",
    "honeypot.outcome.timeout",
    "honeypot.outcome.warning",
];

fn honeypot_samples() -> Vec<Sample> {
    let on = |levels: Vec<Option<LevelAction>>, custom: Option<&str>, cases| Warning::On {
        rule: "Honeypot".to_owned(),
        levels,
        custom: custom.map(str::to_owned),
        cases,
    };
    let mut v = Vec::new();
    for (name, w) in [
        (
            "one outcome (ban), one case",
            on(
                vec![Some(LevelAction::Ban {
                    delete_message_seconds: 86_400,
                })],
                None,
                1,
            ),
        ),
        (
            "many outcomes",
            on(
                vec![
                    None,
                    Some(LevelAction::Timeout {
                        duration_seconds: 3_600,
                    }),
                    Some(LevelAction::Kick),
                    Some(LevelAction::Ban {
                        delete_message_seconds: 0,
                    }),
                ],
                None,
                42,
            ),
        ),
        ("warning only", on(vec![None], None, 0)),
        (
            "custom warning",
            on(
                vec![None],
                Some("**Bots only.** Posting here gets you banned. @everyone"),
                3,
            ),
        ),
        (
            "custom warning, maximal",
            on(vec![None], Some(&long(1_500)), 1_234),
        ),
    ] {
        v.push(covering(
            "honeypot.warning",
            WARNING_IDS,
            name,
            Out::Msg(honeypot::warning(&w)),
        ));
    }
    v.push(msg(
        "honeypot.warning.off",
        "",
        honeypot::warning(&Warning::Off),
    ));
    v
}

pub fn samples() -> Vec<Sample> {
    let mut v = tickets();
    v.extend(honeypot_samples());
    v
}
