//! Evidence (§12.6): adding links and files, viewing items with their
//! notes and spoilered galleries, the messages around a saved message,
//! notes, visibility, removal, the "Add as evidence" picker, and why an
//! item or file couldn't be saved.

use quack_core::evidence::{EvidenceFailure, EvidenceKind, EvidenceState, FileCopyState};
use quack_core::ids::{CaseId, ChannelId, EvidenceId, MessageId};
use quack_core::limits::{EVIDENCE_NOTE, MAX_UPLOAD_FILES};
use time::OffsetDateTime;

use crate::actions::{ButtonAction, InputId, ModalAction, SelectAction, SelectValue};
use crate::fmt;
use crate::model::{
    Block, Button, ButtonStyle, ChannelRef, GalleryItem, Input, InputKind, InputStyle, Modal, Msg,
    Person, Select, SelectOption, SelectOptions, Text, TimeStyle, Tone,
};

/// The note input, shared by the add-evidence and note modals.
fn note_input(value: Option<&str>) -> Input {
    Input {
        id: InputId::Note,
        label: "Note".to_owned(),
        description: Some("Optional. Only staff see it.".to_owned()),
        required: false,
        kind: InputKind::Text {
            style: InputStyle::Paragraph,
            placeholder: Some("What this shows, or where it came from".to_owned()),
            min_length: 0,
            max_length: u16::try_from(EVIDENCE_NOTE.max).unwrap_or(500),
            value: value.map(str::to_owned),
        },
    }
}

/// `evidence.modal.*`: the add-evidence modal: message links, files, and
/// a note. At least one link or file is required (checked on submit).
pub fn modal(case: CaseId, number: u64) -> Modal {
    Modal {
        action: ModalAction::AddEvidence(case),
        title: format!("Add evidence · case #{number}"),
        inputs: vec![
            Input {
                id: InputId::Links,
                label: "Message links".to_owned(),
                description: Some("One Discord message link per line, up to 10.".to_owned()),
                required: false,
                kind: InputKind::Text {
                    style: InputStyle::Paragraph,
                    placeholder: Some("https://discord.com/channels/…".to_owned()),
                    min_length: 0,
                    max_length: 4_000,
                    value: None,
                },
            },
            Input {
                id: InputId::Files,
                label: "Files".to_owned(),
                description: Some(
                    "Up to 10 images, videos, audio, text, or PDFs, 10 MB each.".to_owned(),
                ),
                required: false,
                kind: InputKind::Files {
                    min: 0,
                    max: u8::try_from(MAX_UPLOAD_FILES).unwrap_or(10),
                },
            },
            note_input(None),
        ],
    }
}

/// What one add-evidence call did, for [`added`] and [`added_reply`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Added {
    /// The case's number.
    pub number: u64,
    /// New message items.
    pub new: u32,
    /// Failed items captured again.
    pub retrying: u32,
    /// Removed items shown again.
    pub restored: u32,
    /// Items already saved.
    pub already_saved: u32,
    /// Files uploaded as one new item.
    pub files: u32,
}

/// `evidence.added`, `evidence.added.files`, `evidence.added.retrying`,
/// `evidence.added.restored`, and `evidence.added.already_saved`: what
/// adding evidence did.
pub fn added(a: &Added) -> Text {
    let number = a.number;
    let mut t = Text::new();
    let messages = |n: u32| fmt::plural(u64::from(n), "message", "messages");
    if a.new > 0 {
        t = t.s(format!(
            "Quack is saving {} as evidence for case #{number}.",
            messages(a.new)
        ));
    }
    if a.files > 0 {
        t = t.sentence(Text::fixed(format!(
            "Quack is copying {} to the evidence channel for case #{number}. Only staff see them until someone selects Show to member.",
            fmt::plural(u64::from(a.files), "file", "files")
        )));
    }
    if a.retrying > 0 {
        t = t.sentence(Text::fixed(format!(
            "Quack will try again to save {} that it couldn't save before.",
            messages(a.retrying)
        )));
    }
    if a.restored > 0 {
        t = t.sentence(Text::fixed(format!(
            "{} that had been removed {} back.",
            fmt::upper_first(&messages(a.restored)),
            if a.restored == 1 { "is" } else { "are" }
        )));
    }
    if a.new + a.retrying + a.files > 0 {
        t = t.s(" Select View evidence to see the outcome.");
    }
    if a.already_saved > 0 {
        t = t.sentence(Text::fixed(match a.already_saved {
            1 => "1 message was already saved as evidence.".to_owned(),
            n => format!("{} messages were already saved as evidence.", fmt::count(n)),
        }));
    }
    t
}

/// `button.add_evidence`.
pub fn add_button(case: CaseId) -> Button {
    Button::new(
        "Add evidence",
        ButtonStyle::Secondary,
        ButtonAction::AddEvidence(case),
    )
}

/// `button.view_evidence`.
pub fn view_button(case: CaseId) -> Button {
    Button::new(
        "View evidence",
        ButtonStyle::Secondary,
        ButtonAction::ViewEvidence { case, item: 1 },
    )
}

/// The `evidence.added` reply with Add evidence and View evidence, so
/// several things can be added in a row.
pub fn added_reply(case: CaseId, a: &Added) -> Msg {
    Msg::text(Tone::Success, added(a)).buttons(vec![add_button(case), view_button(case)])
}

/// `evidence.failure.*`: why an item couldn't be saved.
pub fn failure(f: EvidenceFailure) -> Text {
    let fixed = match f {
        EvidenceFailure::MessageDeleted => {
            "The message was deleted before Quack saved it. If another message shows what happened, add its link."
        }
        EvidenceFailure::ModeratorNoAccess => {
            "Quack didn't save this message because the moderator who added it can't read the channel. Ask a moderator who can read the channel to add it."
        }
        EvidenceFailure::QuackNoAccess => {
            "Quack couldn't save this message because it can't read the channel. Ask an administrator to allow View Channel and Read Message History for Quack there, then add the link again."
        }
        EvidenceFailure::OtherServer => {
            "Quack didn't save this message because it's from another server. Add a message link from this server."
        }
        EvidenceFailure::DiscordError => {
            "Quack couldn't save this message. Add the link again to retry."
        }
        EvidenceFailure::NoFileCopied => {
            return Text::fixed(
                "Quack couldn't copy any of these files to the evidence channel, so none were saved. Check the evidence channel in ",
            )
            .command(crate::model::Command::Setup)
            .s(", then upload the files again.");
        }
    };
    Text::fixed(fixed)
}

/// An item's state as a sentence (the API's `state_text`).
pub fn state_text(state: EvidenceState, f: Option<EvidenceFailure>) -> Text {
    match state {
        EvidenceState::Saving => Text::fixed("Quack is still saving this evidence."),
        EvidenceState::Saved => Text::fixed("Saved."),
        EvidenceState::Failed => failure(f.unwrap_or(EvidenceFailure::DiscordError)),
    }
}

/// `evidence.file.*`: one file and what happened to its copy.
pub fn file(name: &str, state: FileCopyState) -> Text {
    let t = Text::new().user(fmt::one_line(name)).s(" · ");
    t.s(match state {
        FileCopyState::Pending => "Copying file…",
        FileCopyState::Copied => "Copy saved",
        FileCopyState::LinkOnly => "Original link only. No evidence channel is set.",
        FileCopyState::TooLarge => "Original link only. Quack copies files up to 10 MB.",
        FileCopyState::UnsupportedType => "Original link only. Quack doesn't copy this file type.",
        FileCopyState::CopyFailed => {
            "Quack couldn't copy this file. The original link may stop working."
        }
    })
}

/// `evidence.removed_by`: a removed item, as the staff dashboard lists it.
pub fn removed_by(by: &Person, at: OffsetDateTime) -> Text {
    Text::fixed("Removed by ")
        .person(by)
        .s(" ")
        .time(at, TimeStyle::Relative)
        .s(".")
}

/// One evidence item, as the evidence viewer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceItem {
    /// The case.
    pub case: CaseId,
    /// The case's number.
    pub number: u64,
    /// The item.
    pub evidence: EvidenceId,
    /// A saved message or uploaded files.
    pub kind: EvidenceKind,
    /// This item's position among items that aren't removed, 1-based.
    pub item: u32,
    /// How many items that aren't removed the case has.
    pub total: u32,
    /// Where it stands.
    pub state: EvidenceState,
    /// Why it failed.
    pub failure: Option<EvidenceFailure>,
    /// Who added it (`None`: Quack).
    pub added_by: Option<Person>,
    /// When it was added.
    pub added_at: OffsetDateTime,
    /// Who wrote the message, once saved.
    pub author: Option<Person>,
    /// Where it was posted (messages).
    pub channel: Option<ChannelRef>,
    /// When it was sent, once saved.
    pub sent_at: Option<OffsetDateTime>,
    /// When it was last edited.
    pub edited_at: Option<OffsetDateTime>,
    /// The original message (messages).
    pub url: Option<String>,
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
    /// The staff note.
    pub note: Option<String>,
    /// Whether the member sees it (uploads can be staff-only).
    pub visible_to_member: bool,
    /// How many surrounding messages were saved; `None` when none were.
    pub surrounding: Option<u32>,
    /// Copied images and videos with fresh URLs, shown spoilered.
    pub gallery: Vec<(String, String)>,
}

fn heading(e: &EvidenceItem) -> Block {
    Block::Heading(Text::fixed(format!(
        "Evidence {} of {} · case #{}",
        e.item, e.total, e.number
    )))
}

/// The item's first line: the message and where it was, or who uploaded
/// the files.
fn source_line(e: &EvidenceItem) -> Text {
    if e.kind == EvidenceKind::Upload {
        let mut line = Text::fixed("Files uploaded ");
        if let Some(by) = &e.added_by {
            line = line.s("by ").person(by).s(" ");
        }
        return line.time(e.added_at, TimeStyle::Relative).s(".");
    }
    let mut line = Text::fixed("Message ");
    if let Some(a) = &e.author {
        line = line.s("from ").person(a).s(" ");
    }
    if let Some(channel) = &e.channel {
        line = line.s("in ").channel(channel);
    }
    if let Some(at) = e.sent_at {
        line = line.s(", sent ").time(at, TimeStyle::Relative);
    }
    match &e.url {
        Some(url) => line.s(" · ").link("Open message", url),
        None => line,
    }
}

/// `evidence.view.staff_only` / `evidence.view.visible_to_member`.
pub fn visibility_line(visible: bool) -> Text {
    Text::fixed(if visible {
        "The member sees these files on their case page."
    } else {
        "Only staff see these files. The member doesn't see them on their case page."
    })
}

fn saved_body(mut msg: Msg, e: &EvidenceItem) -> Msg {
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
        msg = msg.para(
            Text::fixed(
                "Quack didn't copy files because the evidence channel is visible outside staff. Fix its View Channel permissions or choose another evidence channel in ",
            )
            .command(crate::model::Command::Setup)
            .s("."),
        );
    }
    msg
}

fn view_buttons(e: &EvidenceItem) -> Vec<Button> {
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
    if e.kind == EvidenceKind::Upload {
        let (label, visible) = if e.visible_to_member {
            ("Hide from member", false)
        } else {
            ("Show to member", true)
        };
        buttons.push(Button::new(
            label,
            ButtonStyle::Secondary,
            ButtonAction::EvidenceVisibility {
                evidence: e.evidence,
                visible,
            },
        ));
    }
    buttons.push(Button::new(
        if e.note.is_some() {
            "Edit note"
        } else {
            "Add note"
        },
        ButtonStyle::Secondary,
        ButtonAction::EditEvidenceNote(e.evidence),
    ));
    if e.surrounding.is_some_and(|n| n > 0) {
        buttons.push(Button::new(
            "Messages around it",
            ButtonStyle::Secondary,
            ButtonAction::EvidenceSurrounding(e.evidence),
        ));
    }
    buttons.push(Button::new(
        "Remove",
        ButtonStyle::Danger,
        ButtonAction::RemoveEvidence(e.evidence),
    ));
    buttons.push(crate::word::open_case_button(case));
    buttons
}

/// `evidence.view.*`: one item, with its note, its copied images and
/// videos as a spoilered gallery, and its controls.
pub fn view(e: &EvidenceItem) -> Msg {
    let mut msg = Msg::new(match e.state {
        EvidenceState::Saving => Tone::Warning,
        EvidenceState::Saved => Tone::Neutral,
        EvidenceState::Failed => Tone::Danger,
    })
    .block(heading(e))
    .para(source_line(e));
    if e.kind == EvidenceKind::Upload {
        msg = msg.para(visibility_line(e.visible_to_member));
    }
    if let Some(note) = &e.note {
        msg = msg.block(Block::Field {
            label: "Note".to_owned(),
            value: Text::new().user(note),
        });
    }
    msg = match e.state {
        EvidenceState::Saving => msg.para(state_text(e.state, None)),
        EvidenceState::Failed => msg.para(state_text(e.state, e.failure)),
        EvidenceState::Saved => saved_body(msg, e),
    };
    if e.state == EvidenceState::Saving && e.kind == EvidenceKind::Upload {
        for (name, state) in &e.files {
            msg = msg.para(file(name, *state));
        }
    }
    if !e.gallery.is_empty() {
        msg = msg.block(Block::Gallery(
            e.gallery
                .iter()
                .map(|(name, url)| GalleryItem {
                    url: url.clone(),
                    description: Some(fmt::one_line(name)),
                    spoiler: true,
                })
                .collect(),
        ));
    }
    msg.buttons(view_buttons(e))
}

/// `evidence.view.none`.
pub fn view_none(number: u64) -> Text {
    Text::fixed(format!(
        "Case #{number} has no evidence yet. Use Add evidence to add message links or files."
    ))
}

/// One message saved around a linked message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurroundingLine {
    /// Its author.
    pub author: Person,
    /// When it was sent.
    pub sent_at: OffsetDateTime,
    /// Its text (at most 500 characters).
    pub content: Option<String>,
    /// How many files it had.
    pub files: u32,
    /// Before the linked message (else after).
    pub before: bool,
}

/// The messages around a saved message, for [`surrounding`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surrounding {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// The item's position, for the way back.
    pub item: u32,
    /// The saved message's author, once saved.
    pub author: Option<Person>,
    /// The messages, oldest first.
    pub lines: Vec<SurroundingLine>,
}

fn surrounding_line(msg: Msg, l: &SurroundingLine) -> Msg {
    let mut head = Text::new()
        .person(&l.author)
        .s(" · ")
        .time(l.sent_at, TimeStyle::Relative);
    if l.files > 0 {
        head = head.s(format!(
            " · {}",
            fmt::plural(u64::from(l.files), "file", "files")
        ));
    }
    let msg = msg.para(head);
    match l.content.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(text) => msg.block(Block::Quote(Text::new().user(text))),
        None => msg.block(Block::Subtext(Text::fixed("No text."))),
    }
}

/// `evidence.surrounding.*`: up to 5 messages before and 5 after a saved
/// message, text only (staff only).
pub fn surrounding(s: &Surrounding) -> Msg {
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(Text::fixed(format!(
            "Messages around the evidence · case #{}",
            s.number
        ))))
        .block(Block::Subtext(Text::fixed(
            "Saved when the message was added, text only. Only staff see them.",
        )));
    if s.lines.is_empty() {
        msg = msg.para(Text::fixed(
            "There were no other messages around it when Quack saved it.",
        ));
    }
    let mut marked = false;
    for l in &s.lines {
        if !l.before && !marked {
            msg = mark_saved(msg, s);
            marked = true;
        }
        msg = surrounding_line(msg, l);
    }
    if !marked && !s.lines.is_empty() {
        msg = mark_saved(msg, s);
    }
    msg.buttons(vec![
        Button::new(
            "Back to evidence",
            ButtonStyle::Secondary,
            ButtonAction::ViewEvidence {
                case: s.case,
                item: s.item,
            },
        ),
        crate::word::open_case_button(s.case),
    ])
}

fn mark_saved(msg: Msg, s: &Surrounding) -> Msg {
    let mut t = Text::fixed("The saved message");
    if let Some(a) = &s.author {
        t = t.s(" from ").person(a);
    }
    msg.block(Block::Divider)
        .para(t.s("."))
        .block(Block::Divider)
}

/// `evidence.remove.confirm`: asks before removing an item.
pub fn remove_confirm(case: CaseId, number: u64, evidence: EvidenceId, item: u32) -> Msg {
    Msg::text(
        Tone::Warning,
        Text::fixed(format!(
            "Remove this evidence item from case #{number}? Staff won't see it in Discord and the member won't see it on their case page. The dashboard still lists it as removed, and its copies stay in the evidence channel."
        )),
    )
    .buttons(vec![
        Button::new(
            "Yes, remove",
            ButtonStyle::Danger,
            ButtonAction::ConfirmRemoveEvidence(evidence),
        ),
        Button::new(
            "Back",
            ButtonStyle::Secondary,
            ButtonAction::ViewEvidence { case, item },
        ),
    ])
}

/// `evidence.removed` / `evidence.removed.already`.
pub fn removed(number: u64, changed: bool) -> Text {
    Text::fixed(if changed {
        format!(
            "Quack removed the evidence item from case #{number}. A removed message comes back when its link is added again; removed files have to be uploaded again."
        )
    } else {
        format!("That evidence item was already removed from case #{number}.")
    })
}

/// `evidence.visibility.shown` / `evidence.visibility.hidden`.
pub fn visibility_changed(number: u64, visible: bool) -> Text {
    Text::fixed(if visible {
        format!("The member can now see these files on their page for case #{number}.")
    } else {
        format!(
            "Only staff can see these files now. They're hidden from the member's page for case #{number}."
        )
    })
}

/// `evidence.note.modal`: the note modal, prefilled with the current note.
pub fn note_modal(evidence: EvidenceId, number: u64, revision: u32, note: Option<&str>) -> Modal {
    Modal {
        action: ModalAction::EvidenceNote { evidence, revision },
        title: format!("Note · case #{number}"),
        inputs: vec![note_input(note)],
    }
}

/// `evidence.note.saved` / `evidence.note.cleared`.
pub fn note_saved(number: u64, cleared: bool) -> Text {
    Text::fixed(if cleared {
        format!("Quack removed the note from this evidence item of case #{number}.")
    } else {
        format!("Quack saved the note on this evidence item of case #{number}.")
    })
}

/// One case the "Add as evidence" picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerCase {
    /// The case.
    pub case: CaseId,
    /// Its number.
    pub number: u64,
    /// Its rule's name (`None` for imported cases).
    pub rule: Option<String>,
    /// When it was added.
    pub created_at: OffsetDateTime,
    /// Whether it is voided.
    pub voided: bool,
}

/// `evidence.picker.*`: the "Add as evidence" message command's case
/// picker, newest first, and a button to enter a case number.
pub fn picker(channel: ChannelId, message: MessageId, cases: &[PickerCase]) -> Msg {
    let mut msg = Msg::new(Tone::Neutral)
        .block(Block::Heading(Text::fixed("Add this message as evidence")))
        .para(Text::fixed(if cases.is_empty() {
            "Quack found no recent cases about this message's author or added by you. Enter a case number instead."
        } else {
            "Choose a case about this message's author or one you added recently, or enter a case number."
        }));
    if !cases.is_empty() {
        let options = cases
            .iter()
            .take(crate::budget::SELECT_OPTIONS)
            .map(|c| {
                let rule = c.rule.as_deref().unwrap_or(crate::word::IMPORTED);
                let label = format!("#{} · {}", c.number, fmt::one_line(rule));
                let when = format!("Added {}", fmt::date(c.created_at));
                let description = if c.voided {
                    format!("Voided · {when}")
                } else {
                    when
                };
                SelectOption::new(&label, Some(&description), SelectValue::Case(c.case))
            })
            .collect();
        msg = msg.select(Select {
            action: SelectAction::AddAsEvidence { channel, message },
            placeholder: "Choose a case".to_owned(),
            options: SelectOptions::Strings(options),
        });
    }
    msg.buttons(vec![Button::new(
        "Enter a case number",
        ButtonStyle::Secondary,
        ButtonAction::AddAsEvidenceByNumber { channel, message },
    )])
}

/// `evidence.picker.modal`: the case number and an optional note.
pub fn picker_modal(channel: ChannelId, message: MessageId) -> Modal {
    Modal {
        action: ModalAction::AddAsEvidence { channel, message },
        title: "Add as evidence".to_owned(),
        inputs: vec![
            Input {
                id: InputId::CaseNumber,
                label: "Case number".to_owned(),
                description: Some("The number on the case's receipt.".to_owned()),
                required: true,
                kind: InputKind::Text {
                    style: InputStyle::Short,
                    placeholder: Some("12".to_owned()),
                    min_length: 1,
                    max_length: 16,
                    value: None,
                },
            },
            note_input(None),
        ],
    }
}
