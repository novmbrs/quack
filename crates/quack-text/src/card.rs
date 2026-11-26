//! Cards: the neutral picture model for the ops images (§21.6, §21.8).
//!
//! A [`Card`] holds the words and numbers of one image, top to bottom;
//! `quack-card` draws it. Like a [`Msg`](crate::Msg), it never holds text
//! a person typed, only Quack's own words and counts. Its
//! [`Card::alt`] is the image's description for screen readers and the
//! plain-text fallback.

use crate::model::Tone;

/// One image: a header, sections, and a footer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// The large first line.
    pub title: String,
    /// The small line under it (version, uptime).
    pub subtitle: Option<String>,
    /// The tone of the accent stripe and the badge.
    pub tone: Tone,
    /// A short status pill at the top right ("Operational").
    pub badge: Option<String>,
    /// The sections, top to bottom.
    pub sections: Vec<Section>,
    /// The footer's left words.
    pub footer: String,
    /// The footer's right words (usually when it was read).
    pub footer_right: Option<String>,
    /// What the image says, in one or two sentences.
    pub alt: String,
}

/// One section of a [`Card`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    /// Big numbers in a row (up to four per row; more wrap).
    Tiles(Vec<Tile>),
    /// Named parts and how each is doing, one per line.
    Checks {
        /// The title over the lines, if any.
        title: Option<String>,
        /// The lines.
        rows: Vec<Check>,
    },
    /// A column chart over time.
    Columns(Columns),
    /// Horizontal bars, one per label.
    Bars {
        /// The title over the bars.
        title: String,
        /// The bars.
        rows: Vec<Bar>,
        /// What a full bar means (a share of this); the longest bar when
        /// `None`.
        out_of: Option<u64>,
    },
    /// An aligned table: the first column left, the rest right.
    Table {
        /// The title over the table.
        title: String,
        /// The column headings.
        header: Vec<String>,
        /// The rows, as many cells as the header.
        rows: Vec<Vec<String>>,
    },
}

/// One big number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    /// The small label over it ("Servers").
    pub label: String,
    /// The number as shown ("288K").
    pub value: String,
    /// A small note under it, if any.
    pub note: Option<String>,
    /// A dot by the label: green, amber, or red; none when neutral.
    pub tone: Tone,
}

impl Tile {
    /// A tile without a note or a dot.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            note: None,
            tone: Tone::Neutral,
        }
    }

    /// The same tile with a note.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// The same tile with a dot.
    #[must_use]
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

/// One part and how it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// The part ("Discord gateway").
    pub label: String,
    /// A muted detail after the label ("40 ms heartbeat").
    pub detail: Option<String>,
    /// The state on the right ("Operational"), colored by `tone`.
    pub state: String,
    /// Green, amber, or red.
    pub tone: Tone,
}

/// A column chart: one column per period, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns {
    /// The title at the left.
    pub title: String,
    /// A note at the right ("15 in 24 hours").
    pub note: Option<String>,
    /// The values, oldest first.
    pub values: Vec<u64>,
    /// Part of each value drawn red at its base (failures), if any.
    pub failed: Option<Vec<u64>>,
    /// The label under the first column ("24h ago").
    pub start: String,
    /// The label under the last column ("now").
    pub end: String,
}

/// One horizontal bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    /// The label at the left.
    pub label: String,
    /// The bar's length, against the longest in its section.
    pub value: u64,
    /// The words at the right ("17% · 210").
    pub shown: String,
}
