//! Channels `/setup` creates (§12.11 `channel.*`) and the setup modals.

use quack_core::limits;

use crate::actions::{InputId, ModalAction};
use crate::budget;
use crate::fmt;
use crate::model::{Block, Input, InputKind, InputStyle, Modal, Msg, Text, Tone};

/// A channel Quack can create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    /// `#moderation-log`.
    Log,
    /// `#appeals`.
    Appeals,
    /// `#evidence`.
    Evidence,
    /// `#support`, where members open tickets.
    TicketEntry,
    /// `#ticket-log`, the ticket staff channel.
    TicketQueue,
    /// `#honeypot`.
    Honeypot,
    /// `#server-log`.
    ActivityLog,
}

/// The new channel's name.
pub const fn channel_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Log => "moderation-log",
        ChannelKind::Appeals => "appeals",
        ChannelKind::Evidence => "evidence",
        ChannelKind::TicketEntry => "support",
        ChannelKind::TicketQueue => "ticket-log",
        ChannelKind::Honeypot => "honeypot",
        ChannelKind::ActivityLog => "server-log",
    }
}

/// The new channel's topic.
pub const fn channel_topic(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Log => "Quack's moderation log: cases, actions, appeals, and setting changes.",
        ChannelKind::Appeals => "Appeals for moderators to review.",
        ChannelKind::Evidence => "Copies of evidence files for Quack cases.",
        ChannelKind::TicketEntry => "Need help from the staff? Open a ticket below.",
        ChannelKind::TicketQueue => "New tickets and their transcripts.",
        ChannelKind::Honeypot => "Don't post here. Read the warning below.",
        ChannelKind::ActivityLog => {
            "Message edits and deletions, members joining and leaving, bans, and server changes."
        }
    }
}

/// The first message Quack posts in a new channel. `None` for the ticket
/// entry channel (the ticket panel) and the honeypot (the warning), which
/// their modules post.
pub fn first_message(kind: ChannelKind) -> Option<Msg> {
    let (heading, body) = match kind {
        ChannelKind::Log => (
            "Moderation log",
            "Quack posts moderation log entries here. Only staff can see this channel.",
        ),
        ChannelKind::Appeals => (
            "Appeals",
            "New appeals appear here. Accept or reject them here or on the dashboard.",
        ),
        ChannelKind::Evidence => (
            "Evidence",
            "Quack copies evidence files here so they last after the original message is deleted. Please don't delete messages here.",
        ),
        ChannelKind::TicketQueue => (
            "Tickets",
            "New tickets appear here. Open a ticket's thread to reply.",
        ),
        ChannelKind::ActivityLog => (
            "Server activity log",
            "Quack posts server activity here. Only staff can see this channel.",
        ),
        ChannelKind::TicketEntry | ChannelKind::Honeypot => return None,
    };
    Some(
        Msg::new(Tone::Neutral)
            .block(Block::Heading(Text::fixed(heading)))
            .para(Text::fixed(body)),
    )
}

/// `setup.invite.modal.*`, prefilled with the current invite.
pub fn invite_modal(current: Option<&str>) -> Modal {
    Modal {
        action: ModalAction::SetupInvite,
        title: "Rejoin invite".to_owned(),
        inputs: vec![Input {
            id: InputId::Invite,
            label: "Invite link".to_owned(),
            description: Some(
                "Paste a Discord invite link. Quack includes it in DMs about accepted appeals for kicks and bans."
                    .to_owned(),
            ),
            required: true,
            kind: InputKind::Text {
                style: InputStyle::Short,
                placeholder: Some("https://discord.gg/abc123".to_owned()),
                min_length: 1,
                max_length: 200,
                value: current.map(|s| fmt::cut(s, 200)),
            },
        }],
    }
}

/// `setup.warning.modal.*`, prefilled with the current custom warning.
pub fn warning_modal(current: Option<&str>) -> Modal {
    let max = limits::HONEYPOT_WARNING.max.min(budget::INPUT_MAX_LENGTH);
    Modal {
        action: ModalAction::SetupWarning,
        title: "Honeypot warning".to_owned(),
        inputs: vec![Input {
            id: InputId::Warning,
            label: "Warning text".to_owned(),
            description: Some(
                "Shown in the honeypot channel. Leave it empty for Quack's warning, which lists the rule's actions."
                    .to_owned(),
            ),
            required: false,
            kind: InputKind::Text {
                style: InputStyle::Paragraph,
                placeholder: None,
                min_length: 0,
                max_length: u16::try_from(max).unwrap_or(u16::MAX),
                value: current.map(|s| fmt::cut(s, max)),
            },
        }],
    }
}
