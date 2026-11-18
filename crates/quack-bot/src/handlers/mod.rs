//! One small function per interaction, grouped by what they act on. Each
//! reads like the design's step list: acknowledge, authorize, call the
//! engine, answer.

pub(crate) mod add_menu;
pub(crate) mod appeals;
pub(crate) mod case_add;
pub(crate) mod case_edit;
pub(crate) mod case_list;
pub(crate) mod case_view;
pub(crate) mod case_void;
pub(crate) mod controls;
pub(crate) mod drafts;
pub(crate) mod help;
pub(crate) mod member_appeal;
pub(crate) mod module;
pub(crate) mod outcome;
pub(crate) mod rules;
pub(crate) mod setup;
mod setup_channels;
mod setup_modules;

use quack_discord::interaction::{Interaction, Person};
use quack_engine::OpKey;

/// The operation key of a write this interaction makes (§4.3): the
/// operation and the interaction id.
pub(crate) fn op_key(ix: &Interaction, operation: &str) -> OpKey {
    OpKey::new(operation, ix.id.to_string())
}

/// A person from the payload, named as members see them.
pub(crate) fn person(p: &Person) -> quack_text::Person {
    let name = p
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| p.account.global_name.clone())
        .unwrap_or_else(|| p.account.username.clone());
    quack_text::Person::new(p.account.id, name)
}
