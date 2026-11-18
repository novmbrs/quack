//! Tickets (§12.13): the panel, opening and closing replies, the staff
//! post, the closing DM, and the plain-text transcript.

use quack_core::ids::TicketId;
use quack_core::modules::{TicketState, TranscriptPartial, ticket_thread_name};
use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{
    Actor, Block, Button, ButtonStyle, ChannelRef, Msg, Person, Text, TimeStyle, Tone,
};

/// `ticket.panel.*`: the panel in the entry channel.
pub fn panel(on: bool) -> Msg {
    let msg =
        Msg::new(Tone::Neutral).block(Block::Heading(Text::fixed("Need help from the staff?")));
    if on {
        msg.para(Text::fixed(
            "Open a ticket to talk privately with the moderators.",
        ))
        .buttons(vec![Button::new(
            "Open a ticket",
            ButtonStyle::Primary,
            ButtonAction::OpenTicket,
        )])
    } else {
        msg.para(Text::fixed("Tickets are turned off right now."))
    }
}

/// `ticket.panel.moved`: the old panel after the entry channel changed.
pub fn panel_moved(channel: &ChannelRef) -> Msg {
    Msg::text(
        Tone::Neutral,
        Text::fixed("Tickets moved to ")
            .channel(channel)
            .s(". Open a ticket there."),
    )
}

/// `ticket.open.opening`.
pub fn open_opening(queue: &ChannelRef) -> Text {
    Text::fixed("Quack is opening your ticket. You'll be added to a private thread in ")
        .channel(queue)
        .s(" in a moment, and Quack will mention you there.")
}

/// `ticket.open.exists`.
pub fn open_exists(thread: &ChannelRef) -> Text {
    Text::fixed("You already have an open ticket: ")
        .channel(thread)
        .s(".")
}

/// `ticket.open.closing`.
pub fn open_closing() -> Text {
    Text::fixed("Your last ticket is still closing. Try again in a minute.")
}

/// `ticket.open.disabled`.
pub fn open_disabled() -> Text {
    Text::fixed("Tickets are turned off in this server.")
}

/// `ticket.open.failed`.
pub fn open_failed(queue: &ChannelRef) -> Text {
    Text::fixed("Quack couldn't open your ticket because Discord didn't let it create the thread. Ask a server administrator to check Quack's permissions in ")
        .channel(queue)
        .s(".")
}

/// `ticket.welcome`: the first message in the thread. The only message
/// whose mention pings (its member).
pub fn welcome(member: &Person, ticket: TicketId) -> Msg {
    Msg::text(
        Tone::Neutral,
        Text::new()
            .person(member)
            .s(", your ticket is open. Tell us what you need help with, and a moderator will reply here."),
    )
    .buttons(vec![close_button(ticket)])
}

/// `ticket.button.close`.
pub fn close_button(ticket: TicketId) -> Button {
    Button::new(
        "Close ticket",
        ButtonStyle::Danger,
        ButtonAction::CloseTicket(ticket),
    )
}

/// How the staff post points at the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptPlace {
    /// Not saved yet.
    NotYet,
    /// Attached to the post.
    Attached,
    /// On the dashboard until this time.
    Dashboard(OffsetDateTime),
    /// Kept until this time and deleted since.
    Expired(OffsetDateTime),
}

/// Everything the ticket's staff post shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketPost {
    /// The ticket.
    pub ticket: TicketId,
    /// The member who opened it.
    pub member: Person,
    /// Where it stands.
    pub state: TicketState,
    /// Opening failed (the ticket closed without opening).
    pub open_failed: bool,
    /// Its thread, once created.
    pub thread: Option<ChannelRef>,
    /// A link to the thread.
    pub thread_url: Option<String>,
    /// When it opened.
    pub opened_at: OffsetDateTime,
    /// Who closed it and when.
    pub closed: Option<(Actor, OffsetDateTime)>,
    /// Where the transcript is.
    pub transcript: TranscriptPlace,
    /// Why the transcript may be missing messages.
    pub partial: Option<TranscriptPartial>,
    /// Quack couldn't delete the thread.
    pub thread_not_deleted: bool,
}

/// `ticket.post.partial.*`.
const fn partial_cause(p: TranscriptPartial) -> &'static str {
    match p {
        TranscriptPartial::History => "Quack couldn't read the thread's full history",
        TranscriptPartial::Trimmed => "it was cut to 8 MB",
    }
}

/// `ticket.post.*`: the staff post in the ticket staff channel.
pub fn post(p: &TicketPost) -> Msg {
    let mut msg = Msg::new(match p.state {
        TicketState::Opening | TicketState::Closing => Tone::Warning,
        TicketState::Open => Tone::Success,
        TicketState::Closed => Tone::Neutral,
    })
    .block(Block::Heading(
        Text::fixed("Ticket from ").person(&p.member),
    ));
    let state = match p.state {
        TicketState::Opening => Text::fixed("Opening…"),
        TicketState::Open => {
            let t = Text::fixed("Open · ");
            let t = match &p.thread {
                Some(th) => t.channel(th),
                None => t.s("its thread"),
            };
            t.s(" · opened ").time(p.opened_at, TimeStyle::Relative)
        }
        TicketState::Closing => Text::fixed("Closing. Quack is saving the transcript."),
        TicketState::Closed if p.open_failed => Text::fixed("Quack couldn't open this ticket. ")
            .person(&p.member)
            .s(" can try again."),
        TicketState::Closed => match &p.closed {
            Some((by, at)) => Text::fixed("Closed by ")
                .actor(by)
                .s(" ")
                .time(*at, TimeStyle::Relative)
                .s("."),
            None => Text::fixed("Closed."),
        },
    };
    msg = msg.para(state);
    if p.state == TicketState::Closed && !p.open_failed {
        match p.transcript {
            TranscriptPlace::NotYet => {}
            TranscriptPlace::Attached => msg = msg.para(Text::fixed("The transcript is attached.")),
            TranscriptPlace::Dashboard(until) => {
                msg = msg.para(
                    Text::fixed("The transcript is on the dashboard until ")
                        .time(until, TimeStyle::Full)
                        .s("."),
                );
            }
            TranscriptPlace::Expired(until) => {
                msg = msg.para(
                    Text::fixed("The transcript was kept until ")
                        .time(until, TimeStyle::Full)
                        .s(" and has been deleted."),
                );
            }
        }
        if let Some(partial) = p.partial {
            msg = msg.para(Text::fixed(format!(
                "The transcript may be missing some messages: {}.",
                partial_cause(partial)
            )));
        }
        if p.thread_not_deleted {
            msg = msg.para(Text::fixed(
                "Quack couldn't delete the thread. A moderator can delete it in Discord.",
            ));
        }
    }
    let mut buttons = Vec::new();
    if p.state == TicketState::Open {
        if let Some(url) = &p.thread_url {
            buttons.push(Button::link("Open thread", url));
        }
        buttons.push(Button::new(
            "Fix access",
            ButtonStyle::Secondary,
            ButtonAction::FixTicketAccess(p.ticket),
        ));
        buttons.push(close_button(p.ticket));
    }
    msg.buttons(buttons)
}

/// `ticket.close.closing`.
pub fn close_closing() -> Text {
    Text::fixed("Quack is closing this ticket and saving the transcript.")
}

/// `ticket.close.not_open`.
pub fn close_not_open() -> Text {
    Text::fixed("This ticket is already closed or closing.")
}

/// `ticket.close.not_allowed`.
pub fn close_not_allowed() -> Text {
    Text::fixed("Only the member who opened this ticket and moderators can close it.")
}

/// `ticket.fix.done`.
pub fn fix_done(member: &Person, removed: u64) -> Text {
    let t = Text::fixed("Quack added ")
        .person(member)
        .s(" to the thread.");
    match removed {
        0 => t,
        1 => t.s(" It removed 1 person who isn't staff."),
        n => t.s(format!(
            " It removed {} people who aren't staff.",
            fmt::count(n)
        )),
    }
}

/// `ticket.fix.failed`.
pub fn fix_failed(queue: &ChannelRef) -> Text {
    Text::fixed("Discord didn't let Quack fix the ticket's access. Check Quack's Manage Threads permission in ")
        .channel(queue)
        .s(".")
}

/// `dm.ticket_closed.*`: the DM to the member, with the transcript file.
pub fn closed_dm(server: &str, transcript_file: &str) -> Msg {
    Msg::new(Tone::Neutral)
        .block(Block::Heading(
            Text::fixed("Your ticket in ")
                .bold_user(server)
                .s(" is closed"),
        ))
        .para(Text::fixed(
            "Here's a copy of the conversation for your records.",
        ))
        .block(Block::File(transcript_file.to_owned()))
}

/// `ticket.thread_name`.
pub fn thread_name(ticket: TicketId) -> String {
    ticket_thread_name(ticket)
}

/// `ticket.transcript.header`.
pub fn transcript_header(
    server: &str,
    opened: OffsetDateTime,
    member: &str,
    member_id: u64,
    closed: OffsetDateTime,
) -> String {
    format!(
        "Ticket transcript · {} · opened {} by {} ({member_id}) · closed {}",
        fmt::one_line(server),
        fmt::transcript_time(opened),
        fmt::one_line(member),
        fmt::transcript_time(closed)
    )
}

/// `ticket.transcript.line`.
pub fn transcript_line(at: OffsetDateTime, author: &str, author_id: u64, text: &str) -> String {
    format!(
        "[{}] {} ({author_id}): {}",
        fmt::transcript_time(at),
        fmt::one_line(author),
        fmt::clean(text)
    )
}

/// `ticket.transcript.file_line`.
pub fn transcript_file_line(name: &str, size_bytes: u64) -> String {
    format!(
        "  file: {} ({} bytes)",
        fmt::one_line(name),
        fmt::count(size_bytes)
    )
}

/// `ticket.transcript.partial`.
pub fn transcript_partial(partial: TranscriptPartial) -> String {
    format!("Some messages may be missing: {}.", partial_cause(partial))
}
