//! Guided `/setup` (§11.8, §12.11): the home panel, one panel per area,
//! result lines, channel problems, created channels, and setup modals.

mod channels;
mod results;

use quack_core::modules::ModuleName;

pub use channels::{
    ChannelKind, channel_name, channel_topic, first_message, invite_modal, warning_modal,
};
pub use results::{
    SetupResult, channel_not_staff_only, channel_problem, create_failed_discord,
    create_failed_permission, create_unconfirmed, invite_invalid, module_not_ready, module_problem,
    quack_missing_channel_permissions, result, settings_problem,
};

use crate::actions::{ButtonAction, SelectAction, SetupArea, SetupOp};
use crate::case::PickerRule;
use crate::fmt;
use crate::model::{
    Block, Button, ButtonStyle, ChannelRef, Msg, Select, SelectOption, SelectOptions, Text, Tone,
};

/// `module.*`: a module's name inside a sentence.
pub const fn module_name(m: ModuleName) -> &'static str {
    match m {
        ModuleName::Tickets => "tickets",
        ModuleName::ActivityLog => "the server activity log",
        ModuleName::Honeypot => "the honeypot",
    }
}

/// Where the server activity log posts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ActivityRoutes {
    /// No channel yet.
    #[default]
    None,
    /// Every event goes to one channel.
    One(ChannelRef),
    /// Events go to different channels.
    Several,
}

/// The settings `/setup` shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one on/off flag per module")]
pub struct SetupState {
    /// The log channel.
    pub log: Option<ChannelRef>,
    /// The appeal channel.
    pub appeals: Option<ChannelRef>,
    /// Whether a rejoin invite is set.
    pub invite_set: bool,
    /// The evidence channel.
    pub evidence: Option<ChannelRef>,
    /// Whether tickets are on.
    pub tickets_on: bool,
    /// Where members open tickets.
    pub ticket_entry: Option<ChannelRef>,
    /// Where new tickets go.
    pub ticket_queue: Option<ChannelRef>,
    /// Whether the honeypot is on.
    pub honeypot_on: bool,
    /// The honeypot channel.
    pub honeypot_channel: Option<ChannelRef>,
    /// The honeypot rule's name.
    pub honeypot_rule: Option<String>,
    /// Whether the server activity log is on.
    pub activity_on: bool,
    /// Where it posts.
    pub activity: ActivityRoutes,
    /// The dashboard's settings page.
    pub settings_url: String,
}

fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

fn channel_or(c: Option<&ChannelRef>, none: &str) -> Text {
    c.map_or_else(|| Text::fixed(none), |c| Text::new().channel(c))
}

fn line(label: &str, value: Text) -> Block {
    Block::Field {
        label: label.to_owned(),
        value,
    }
}

/// `setup.home.*` for one area: its label and current state.
fn state_line(area: SetupArea, s: &SetupState) -> Option<Block> {
    Some(match area {
        SetupArea::Home => return None,
        SetupArea::Log => line("Log channel", channel_or(s.log.as_ref(), "Not set")),
        SetupArea::Appeals => {
            let t = channel_or(s.appeals.as_ref(), "Not set");
            line(
                "Appeals",
                if s.invite_set {
                    t.s(", rejoin invite set")
                } else {
                    t
                },
            )
        }
        SetupArea::Evidence => line("Evidence copies", channel_or(s.evidence.as_ref(), "Off")),
        SetupArea::Tickets => {
            let mut t = Text::fixed(on_off(s.tickets_on));
            if let (Some(entry), Some(queue)) = (&s.ticket_entry, &s.ticket_queue) {
                t = t
                    .s(" · members open tickets in ")
                    .channel(entry)
                    .s(" · new tickets go to ")
                    .channel(queue);
            }
            line("Tickets", t)
        }
        SetupArea::Honeypot => {
            let mut t = Text::fixed(on_off(s.honeypot_on));
            if let (Some(c), Some(rule)) = (&s.honeypot_channel, &s.honeypot_rule) {
                t = t.s(" · ").channel(c).s(" · rule ").bold_user(rule);
            }
            line("Honeypot", t)
        }
        SetupArea::ActivityLog => {
            let t = Text::fixed(on_off(s.activity_on));
            line(
                "Server activity log",
                match &s.activity {
                    ActivityRoutes::None => t,
                    ActivityRoutes::One(c) => t.s(" · ").channel(c),
                    ActivityRoutes::Several => t.s(" · several channels"),
                },
            )
        }
    })
}

const AREAS: [(SetupArea, &str); 6] = [
    (SetupArea::Log, "Log channel"),
    (SetupArea::Appeals, "Appeals"),
    (SetupArea::Evidence, "Evidence copies"),
    (SetupArea::Tickets, "Tickets"),
    (SetupArea::Honeypot, "Honeypot"),
    (SetupArea::ActivityLog, "Server activity log"),
];

fn button(label: &str, area: SetupArea, op: SetupOp) -> Button {
    Button::new(label, ButtonStyle::Secondary, ButtonAction::Setup(area, op))
}

fn channel_select(area: SetupArea, op: SetupOp, placeholder: &str) -> Select {
    Select {
        action: SelectAction::Setup(area, op),
        placeholder: placeholder.to_owned(),
        options: SelectOptions::TextChannels,
    }
}

/// `setup.home.*`: the home panel, with the result of the last change.
pub fn home(s: &SetupState, result: Option<Text>) -> Msg {
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(Text::fixed("Set up Quack")));
    if let Some(r) = result {
        msg = msg.para(r);
    }
    msg = msg.para(Text::fixed(
        "Choose what to set up. Changes save right away and also show on the dashboard.",
    ));
    for (area, _) in AREAS {
        msg.blocks.extend(state_line(area, s));
    }
    let mut buttons: Vec<Button> = AREAS
        .iter()
        .map(|(area, label)| button(label, *area, SetupOp::Open))
        .collect();
    buttons.push(Button::link("Open server settings", &s.settings_url));
    msg.buttons(buttons)
}

fn explain(area: SetupArea) -> &'static str {
    match area {
        SetupArea::Home => "",
        SetupArea::Log => "Quack posts moderation log entries in this staff-only channel.",
        SetupArea::Appeals => {
            "New appeals are posted in this staff-only channel for moderators to review. Quack can include a rejoin invite in DMs about accepted appeals for kicks and bans."
        }
        SetupArea::Evidence => {
            "Quack copies evidence files to this staff-only channel so they last after the original message is deleted."
        }
        SetupArea::Tickets => {
            "Members open private tickets from a button in the entry channel. Staff see new tickets in the staff channel."
        }
        SetupArea::Honeypot => {
            "When the honeypot is on, a member who posts in this channel gets a case under the chosen rule. Quack ignores posts from staff and bots."
        }
        SetupArea::ActivityLog => {
            "Quack posts message edits and deletions, members joining and leaving, bans, and server changes here."
        }
    }
}

/// **Create #name**, or **Create a new #name** once a channel is saved for
/// the area: a press then makes a second channel and switches to it
/// (§11.8), which the label says.
fn create(kind: ChannelKind, area: SetupArea, saved: bool) -> Button {
    let label = if saved {
        format!("Create a new #{}", channel_name(kind))
    } else {
        format!("Create #{}", channel_name(kind))
    };
    button(&label, area, SetupOp::Create)
}

/// The tickets area's **Create** button, which makes both channels.
fn create_ticket_channels(s: &SetupState) -> Button {
    let label = if s.ticket_entry.is_some() || s.ticket_queue.is_some() {
        "Create new #support and #ticket-log"
    } else {
        "Create #support and #ticket-log"
    };
    button(label, SetupArea::Tickets, SetupOp::Create)
}

fn toggle(on: bool, area: SetupArea) -> Button {
    if on {
        button("Turn off", area, SetupOp::TurnOff)
    } else {
        button("Turn on", area, SetupOp::TurnOn)
    }
}

/// One area's panel: what it does, its state, its controls, and **Back**.
/// `rules` are the rules the honeypot may use (active, no required fields).
pub fn area(area: SetupArea, s: &SetupState, result: Option<Text>, rules: &[PickerRule]) -> Msg {
    if area == SetupArea::Home {
        return home(s, result);
    }
    let label = AREAS
        .iter()
        .find(|(a, _)| *a == area)
        .map_or("", |(_, l)| l);
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(Text::fixed(label)));
    if let Some(r) = result {
        msg = msg.para(r);
    }
    msg = msg.para(Text::fixed(explain(area)));
    msg.blocks.extend(state_line(area, s));
    let mut buttons = Vec::new();
    match area {
        SetupArea::Home => {}
        SetupArea::Log => {
            msg = msg.select(channel_select(area, SetupOp::Channel, "Choose a channel"));
            buttons.push(create(ChannelKind::Log, area, s.log.is_some()));
            if s.log.is_some() {
                buttons.push(button("Stop posting log entries", area, SetupOp::Clear));
            }
        }
        SetupArea::Appeals => {
            msg = msg.select(channel_select(area, SetupOp::Channel, "Choose a channel"));
            buttons.push(create(ChannelKind::Appeals, area, s.appeals.is_some()));
            if s.appeals.is_some() {
                buttons.push(button("Stop posting appeals", area, SetupOp::Clear));
            }
            buttons.push(button("Set rejoin invite", area, SetupOp::SetInvite));
            if s.invite_set {
                buttons.push(button("Remove invite", area, SetupOp::ClearInvite));
            }
        }
        SetupArea::Evidence => {
            msg = msg.select(channel_select(area, SetupOp::Channel, "Choose a channel"));
            buttons.push(create(ChannelKind::Evidence, area, s.evidence.is_some()));
            if s.evidence.is_some() {
                buttons.push(button("Stop copying files", area, SetupOp::Clear));
            }
        }
        SetupArea::Tickets => {
            msg = msg
                .select(channel_select(
                    area,
                    SetupOp::TicketEntry,
                    "Channel where members open tickets",
                ))
                .select(channel_select(
                    area,
                    SetupOp::TicketQueue,
                    "Staff channel for new tickets",
                ));
            buttons.push(create_ticket_channels(s));
            buttons.push(toggle(s.tickets_on, area));
        }
        SetupArea::Honeypot => {
            msg = msg.select(channel_select(area, SetupOp::Channel, "Choose a channel"));
            if !rules.is_empty() {
                let options = rules
                    .iter()
                    .take(crate::budget::SELECT_OPTIONS)
                    .map(|r| {
                        SelectOption::new(
                            &fmt::one_line(&r.name),
                            Some(&fmt::one_line(&r.description)),
                            crate::actions::SelectValue::Rule(r.id),
                        )
                    })
                    .collect();
                msg = msg.select(Select {
                    action: SelectAction::Setup(area, SetupOp::Rule),
                    placeholder: "Rule the honeypot applies".to_owned(),
                    options: SelectOptions::Strings(options),
                });
            }
            buttons.push(create(
                ChannelKind::Honeypot,
                area,
                s.honeypot_channel.is_some(),
            ));
            if !s.honeypot_on && s.honeypot_rule.is_some() {
                buttons.push(button("Clear rule", area, SetupOp::ClearRule));
            }
            buttons.push(button("Edit warning", area, SetupOp::EditWarning));
            buttons.push(toggle(s.honeypot_on, area));
        }
        SetupArea::ActivityLog => {
            msg = msg.select(channel_select(area, SetupOp::Channel, "Choose a channel"));
            buttons.push(create(
                ChannelKind::ActivityLog,
                area,
                s.activity != ActivityRoutes::None,
            ));
            buttons.push(toggle(s.activity_on, area));
        }
    }
    buttons.push(button("Back", SetupArea::Home, SetupOp::Open));
    msg.buttons(buttons)
}
