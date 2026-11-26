//! Messages: blocks of text, rows of buttons and selects, and modals.

use quack_core::ids::AccountId;

use super::Text;
use crate::actions::{ButtonAction, InputId, ModalAction, SelectAction, SelectValue};
use crate::budget;
use crate::fmt;

/// The accent of a message (the container's colour on Discord).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// No accent.
    #[default]
    Neutral,
    /// Something finished (green).
    Success,
    /// In progress, or worth a look (amber).
    Warning,
    /// Something failed or was refused (red).
    Danger,
}

/// One block of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A heading (`### text`).
    Heading(Text),
    /// A paragraph.
    Text(Text),
    /// A quote: text a person wrote, every line prefixed `> `.
    Quote(Text),
    /// `**label:** value`.
    Field {
        /// The label, without the colon.
        label: String,
        /// The value.
        value: Text,
    },
    /// Small grey text (`-# text`).
    Subtext(Text),
    /// Monospace lines of Quack's own text (an aligned table, bars), in a
    /// code block on Discord. Never anything a person typed.
    Code(String),
    /// A separator line.
    Divider,
    /// A file sent with the message, by its attachment name.
    File(String),
    /// An image sent with the message, shown inline: its attachment name
    /// and a description for screen readers and plain text.
    Image {
        /// The attachment's name.
        name: String,
        /// What the image shows.
        alt: String,
    },
    /// Up to 10 images or videos shown inline from their URLs (fresh CDN
    /// URLs of evidence copies). Evidence galleries are always spoilered.
    Gallery(Vec<GalleryItem>),
}

/// One image or video of a [`Block::Gallery`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GalleryItem {
    /// An `https://` URL of the media.
    pub url: String,
    /// Alt text (the file name), at most 1,024 characters.
    pub description: Option<String>,
    /// Blurred until clicked.
    pub spoiler: bool,
}

/// A button's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// The main thing to do.
    Primary,
    /// Anything else.
    Secondary,
    /// A positive decision (accept).
    Success,
    /// A destructive or negative decision (void, reject).
    Danger,
}

/// What a button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ButtonTarget {
    /// Opens a URL (link button; the style is ignored).
    Link(String),
    /// Sends an interaction the caller maps to a custom id.
    Action(ButtonAction),
}

/// A button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    /// Its label, at most 80 characters (cut by [`Button::new`]).
    pub label: String,
    /// Its look.
    pub style: ButtonStyle,
    /// What it does.
    pub target: ButtonTarget,
}

impl Button {
    /// An action button; the label is cut to Discord's limit.
    pub fn new(label: impl AsRef<str>, style: ButtonStyle, action: ButtonAction) -> Self {
        Self {
            label: fmt::cut(label.as_ref(), budget::BUTTON_LABEL),
            style,
            target: ButtonTarget::Action(action),
        }
    }

    /// A link button; the label is cut to Discord's limit.
    pub fn link(label: impl AsRef<str>, url: impl Into<String>) -> Self {
        Self {
            label: fmt::cut(label.as_ref(), budget::BUTTON_LABEL),
            style: ButtonStyle::Secondary,
            target: ButtonTarget::Link(url.into()),
        }
    }
}

/// One option of a string select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    /// Its label, at most 100 characters.
    pub label: String,
    /// Its description, at most 100 characters.
    pub description: Option<String>,
    /// What choosing it means.
    pub value: SelectValue,
}

impl SelectOption {
    /// An option; label and description are cut to Discord's limits.
    pub fn new(label: &str, description: Option<&str>, value: SelectValue) -> Self {
        Self {
            label: fmt::cut(label, budget::SELECT_OPTION_LABEL),
            description: description
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| fmt::cut(d, budget::SELECT_OPTION_DESCRIPTION)),
            value,
        }
    }
}

/// What a select offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectOptions {
    /// Fixed options (at most 25).
    Strings(Vec<SelectOption>),
    /// The server's text channels, chosen by Discord's channel select.
    TextChannels,
}

/// A select menu in its own row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Select {
    /// What choosing does.
    pub action: SelectAction,
    /// Grey text shown before a choice, at most 150 characters.
    pub placeholder: String,
    /// The choices.
    pub options: SelectOptions,
}

/// A row of controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// Up to 5 buttons.
    Buttons(Vec<Button>),
    /// One select.
    Select(Select),
}

/// A whole message: an accent, text blocks, then rows of controls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    /// The accent.
    pub tone: Tone,
    /// The text, in order.
    pub blocks: Vec<Block>,
    /// Controls under the text, in order.
    pub rows: Vec<Row>,
    /// The line [`budget::fit`] ends a shortened message with, saying where
    /// the rest is; `None` uses `render.trimmed`.
    pub trimmed: Option<Text>,
    /// The one account the message pings (only the ticket welcome, §13.2);
    /// other mentions show but never ping.
    pub ping: Option<AccountId>,
}

impl Msg {
    /// An empty message with an accent.
    pub const fn new(tone: Tone) -> Self {
        Self {
            tone,
            blocks: Vec::new(),
            rows: Vec::new(),
            trimmed: None,
            ping: None,
        }
    }

    /// Lets the message ping `account`.
    #[must_use]
    pub const fn pinging(mut self, account: AccountId) -> Self {
        self.ping = Some(account);
        self
    }

    /// Sets the line a shortened message ends with (where the full details
    /// are), instead of `render.trimmed`.
    #[must_use]
    pub fn when_trimmed(mut self, note: Text) -> Self {
        self.trimmed = Some(note);
        self
    }

    /// A one-paragraph message, such as an error or a confirmation.
    pub fn text(tone: Tone, text: Text) -> Self {
        Self::new(tone).block(Block::Text(text))
    }

    /// Appends a block.
    #[must_use]
    pub fn block(mut self, block: Block) -> Self {
        self.blocks.push(block);
        self
    }

    /// Appends a paragraph.
    #[must_use]
    pub fn para(self, text: Text) -> Self {
        self.block(Block::Text(text))
    }

    /// Appends buttons, five to a row; nothing when `buttons` is empty.
    #[must_use]
    pub fn buttons(mut self, buttons: Vec<Button>) -> Self {
        let mut buttons = buttons.into_iter().peekable();
        while buttons.peek().is_some() {
            let row: Vec<Button> = buttons.by_ref().take(budget::BUTTONS_PER_ROW).collect();
            self.rows.push(Row::Buttons(row));
        }
        self
    }

    /// Appends a select row.
    #[must_use]
    pub fn select(mut self, select: Select) -> Self {
        self.rows.push(Row::Select(select));
        self
    }

    /// The whole message as plain text, one block per line.
    pub fn plain(&self) -> String {
        crate::render::plain_msg(self)
    }
}

/// How a modal text input looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputStyle {
    /// One line.
    Short,
    /// Several lines.
    Paragraph,
}

/// What a modal input takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// Free text.
    Text {
        /// One line or several.
        style: InputStyle,
        /// Grey text shown while empty, at most 100 characters.
        placeholder: Option<String>,
        /// Fewest characters (0 when optional).
        min_length: u16,
        /// Most characters, at most 4,000.
        max_length: u16,
        /// Prefilled text, at most `max_length` characters.
        value: Option<String>,
    },
    /// A Yes / No select, optionally preselected.
    YesNo {
        /// Grey text shown before a choice.
        placeholder: String,
        /// The current answer.
        value: Option<bool>,
    },
    /// A file upload field (Discord's File Upload in a Label): `min`–`max`
    /// files; never prefilled.
    Files {
        /// Fewest files (0 when optional).
        min: u8,
        /// Most files, at most 10.
        max: u8,
    },
}

/// One input of a modal: a label, an optional description, and the input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// Which value this is, for decoding the submit.
    pub id: InputId,
    /// Its label, at most 45 characters.
    pub label: String,
    /// Help text under the label, at most 100 characters.
    pub description: Option<String>,
    /// Whether an answer is required.
    pub required: bool,
    /// The input itself.
    pub kind: InputKind,
}

/// A modal (pop-up form) of 1–5 inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modal {
    /// What submitting does.
    pub action: ModalAction,
    /// Its title, at most 45 characters.
    pub title: String,
    /// The inputs, in order.
    pub inputs: Vec<Input>,
}
