//! A sample of every message, with each meaningful variant.

#![allow(
    clippy::too_many_lines,
    clippy::vec_init_then_push,
    reason = "sample lists are long tables, built one row at a time"
)]

mod actions;
mod activity;
mod appeals;
mod cards;
mod cases;
mod errors;
mod evidence;
mod lists;
mod log_entries;
mod logs;
mod modules;
mod ops;
mod setup;
mod words;

use crate::model::{Modal, Msg, Text};

/// What a message function returned.
pub enum Out {
    /// A whole message.
    Msg(Msg),
    /// A sentence.
    Text(Text),
    /// A modal.
    Modal(Modal),
    /// A label, title, name, or plain-text line, with the limit it must fit.
    Str(String, usize),
}

/// One rendered variant of one message.
pub struct Sample {
    /// The snapshot name: a catalog id, or a group of ids.
    pub id: &'static str,
    /// Which variant.
    pub variant: String,
    /// The output.
    pub out: Out,
    /// Catalog ids this sample shows besides `id`.
    pub covers: &'static [&'static str],
}

pub fn sample(id: &'static str, variant: impl Into<String>, out: Out) -> Sample {
    Sample {
        id,
        variant: variant.into(),
        out,
        covers: &[],
    }
}

pub fn covering(
    id: &'static str,
    covers: &'static [&'static str],
    variant: impl Into<String>,
    out: Out,
) -> Sample {
    Sample {
        id,
        variant: variant.into(),
        out,
        covers,
    }
}

pub fn msg(id: &'static str, variant: impl Into<String>, m: Msg) -> Sample {
    sample(id, variant, Out::Msg(m))
}

pub fn text(id: &'static str, variant: impl Into<String>, t: Text) -> Sample {
    sample(id, variant, Out::Text(t))
}

pub fn modal(id: &'static str, variant: impl Into<String>, m: Modal) -> Sample {
    sample(id, variant, Out::Modal(m))
}

pub fn label(id: &'static str, s: impl Into<String>, limit: usize) -> Sample {
    sample(id, "", Out::Str(s.into(), limit))
}

pub use actions::{action_views as actions_views, dm_views as dms_views};
pub use errors::{api_codes, domain_errors};
pub use log_entries::entries as log_entries;
pub use logs::facts as log_facts;

/// Every sample.
pub fn all() -> Vec<Sample> {
    let mut v = Vec::new();
    v.extend(words::samples());
    v.extend(errors::samples());
    v.extend(cases::samples());
    v.extend(evidence::samples());
    v.extend(lists::samples());
    v.extend(actions::samples());
    v.extend(appeals::samples());
    v.extend(logs::samples());
    v.extend(setup::samples());
    v.extend(modules::samples());
    v.extend(activity::samples());
    v.extend(ops::samples());
    v
}
