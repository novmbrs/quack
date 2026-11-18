//! Evidence (§12.6): adding message links, viewing saved messages, and why
//! a message or file couldn't be saved.

use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::CaseId;
use time::OffsetDateTime;

use crate::actions::{ButtonAction, InputId, ModalAction};
use crate::fmt;
use crate::model::{
    Block, Button, ButtonStyle, ChannelRef, Input, InputKind, InputStyle, Modal, Msg, Person, Text,
    TimeStyle, Tone,
};

/// `evidence.modal.*`: the add-evidence modal.
pub fn modal(case: CaseId, number: u64) -> Modal {
    Modal {
        action: ModalAction::AddEvidence(case),
        title: format!("Add evidence · case #{number}"),
        inputs: vec![Input {
            id: InputId::Links,
            label: "Message links".to_owned(),
            description: Some("One Discord message link per line, up to 10.".to_owned()),
            required: true,
            kind: InputKind::Text {
                style: InputStyle::Paragraph,
                placeholder: Some("https://discord.com/channels/…".to_owned()),
                min_length: 1,
                max_length: 4_000,
                value: None,
            },
        }],
    }
}

/// `evidence.added`, `evidence.added.retrying`, and
/// `evidence.added.already_saved`: what adding links did.
pub fn added(number: u64, new: u32, retrying: u32, already_saved: u32) -> Text {
    let mut t = Text::new();
    if new > 0 {
        t = t.s(match new {
            1 => format!("Quack is saving 1 evidence item for case #{number}."),
            n => format!(
                "Quack is saving {} evidence items for case #{number}.",
                fmt::count(n)
            ),
        });
    }
    if retrying > 0 {
        t = t.sentence(Text::fixed(match retrying {
            1 => "Quack is trying again to save 1 message that failed before.".to_owned(),
            n => format!(
                "Quack is trying again to save {} messages that failed before.",
                fmt::count(n)
            ),
        }));
    }
    if new + retrying > 0 {
        t = t.s(" Open View evidence to check each one.");
    }
    if already_saved > 0 {
        t = t.sentence(Text::fixed(match already_saved {
            1 => "1 of these was already saved.".to_owned(),
            n => format!("{} of these were already saved.", fmt::count(n)),
        }));
    }
    t
}

/// `evidence.failure.*`: why a message couldn't be saved.
pub fn failure(f: EvidenceFailure) -> Text {
    Text::fixed(match f {
        EvidenceFailure::MessageDeleted => {
            "Quack couldn't save this message because it was deleted before Quack fetched it."
        }
        EvidenceFailure::ModeratorNoAccess => {
            "Quack didn't save this message because the moderator who added it can't read that channel."
        }
        EvidenceFailure::QuackNoAccess => {
            "Quack couldn't save this message because it can't read that channel. Give Quack View Channel and Read Message History there, then add the link again to retry."
        }
        EvidenceFailure::OtherServer => {
            "Quack didn't save this message because it's in another server."
        }
        EvidenceFailure::DiscordError => {
            "Quack couldn't save this message because Discord kept failing. Add the link again to retry."
        }
    })
}

/// An evidence item's state as a sentence (the API's `state_text`).
pub fn state_text(state: EvidenceState, f: Option<EvidenceFailure>) -> Text {
    match state {
        EvidenceState::Saving => Text::fixed("Quack is still saving this message."),
        EvidenceState::Saved => Text::fixed("Saved."),
        EvidenceState::Failed => failure(f.unwrap_or(EvidenceFailure::DiscordError)),
    }
}

/// `evidence.file.*`: one file and what happened to its copy.
pub fn file(name: &str, state: FileCopyState) -> Text {
    let t = Text::new().user(fmt::one_line(name)).s(" · ");
    t.s(match state {
        FileCopyState::Pending => "saving…",
        FileCopyState::Copied => "saved copy",
        FileCopyState::LinkOnly => "original link only, because no evidence channel is set",
        FileCopyState::TooLarge => "original link only, because Quack copies files up to 10 MB",
        FileCopyState::UnsupportedType => {
            "original link only, because Quack copies only images, video, audio, text, and PDF files"
        }
        FileCopyState::CopyFailed => {
            "Quack couldn't copy this file, so the original link may stop working"
        }
    })
}

/// One saved message, as the evidence viewer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceItem {
    /// The case.
    pub case: CaseId,
    /// The case's number.
    pub number: u64,
    /// This item's position, 1-based.
    pub item: u32,
    /// How many items the case has.
    pub total: u32,
    /// Where it stands.
    pub state: EvidenceState,
    /// Why it failed.
    pub failure: Option<EvidenceFailure>,
    /// Who wrote the message, once saved.
    pub author: Option<Person>,
    /// Where it was posted.
    pub channel: ChannelRef,
    /// When it was sent, once saved.
    pub sent_at: Option<OffsetDateTime>,
    /// When it was last edited.
    pub edited_at: Option<OffsetDateTime>,
    /// The original message.
    pub url: String,
    /// The saved text.
    pub content: Option<String>,
    /// Link preview titles or types.
    pub embeds: Vec<String>,
    /// Files and their copy state.
    pub files: Vec<(String, FileCopyState)>,
    /// Files over the per-message or per-case cap.
    pub files_not_saved: u32,
    /// The evidence channel stopped being staff-only, so files weren't copied.
    pub channel_not_staff_only: bool,
}

/// `evidence.view.*`: one item, with a pager.
pub fn view(e: &EvidenceItem) -> Msg {
    let mut msg = Msg::new(match e.state {
        EvidenceState::Saving => Tone::Warning,
        EvidenceState::Saved => Tone::Neutral,
        EvidenceState::Failed => Tone::Danger,
    })
    .block(Block::Heading(Text::fixed(format!(
        "Evidence {} of {} · case #{}",
        e.item, e.total, e.number
    ))));
    let mut line = Text::fixed("Message ");
    if let Some(a) = &e.author {
        line = line.s("from ").person(a).s(" ");
    }
    line = line.s("in ").channel(&e.channel);
    if let Some(at) = e.sent_at {
        line = line.s(", sent ").time(at, TimeStyle::Relative);
    }
    msg = msg.para(line.s(" · ").link("Open message", &e.url));
    match e.state {
        EvidenceState::Saving => msg = msg.para(state_text(e.state, None)),
        EvidenceState::Failed => msg = msg.para(state_text(e.state, e.failure)),
        EvidenceState::Saved => {
            if let Some(at) = e.edited_at {
                msg = msg.para(Text::fixed("Edited ").time(at, TimeStyle::Relative).s("."));
            }
            if let Some(text) = e.content.as_deref().filter(|t| !t.trim().is_empty()) {
                msg = msg.block(Block::Quote(Text::new().user(text)));
            }
            if !e.embeds.is_empty() {
                let names: Vec<String> = e.embeds.iter().map(|s| fmt::one_line(s)).collect();
                msg = msg.para(Text::fixed("Link previews: ").user(names.join(", ")));
            }
            for (name, state) in &e.files {
                msg = msg.para(file(name, *state));
            }
            if e.files_not_saved > 0 {
                msg = msg.para(Text::fixed(match e.files_not_saved {
                    1 => "1 more file wasn't saved, because a case keeps at most 100 files and 10 per message.".to_owned(),
                    n => format!(
                        "{} more files weren't saved, because a case keeps at most 100 files and 10 per message.",
                        fmt::count(n)
                    ),
                }));
            }
            if e.channel_not_staff_only {
                msg = msg.para(Text::fixed(
                    "The evidence channel is no longer staff-only, so Quack didn't copy files there.",
                ));
            }
        }
    }
    let case = e.case;
    let mut buttons = Vec::new();
    if e.item > 1 {
        buttons.push(Button::new(
            "Previous",
            ButtonStyle::Secondary,
            ButtonAction::ViewEvidence {
                case,
                item: e.item - 1,
            },
        ));
    }
    if e.item < e.total {
        buttons.push(Button::new(
            "Next",
            ButtonStyle::Secondary,
            ButtonAction::ViewEvidence {
                case,
                item: e.item + 1,
            },
        ));
    }
    buttons.push(crate::word::open_case_button(case));
    msg.buttons(buttons)
}

/// `evidence.view.none`.
pub fn view_none(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} has no evidence yet. Use Add evidence to add message links."
    ))
}
