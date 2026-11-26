//! Every sentence a person reads from Quack.
//!
//! A small neutral message model ([`Msg`], [`Text`], buttons, selects,
//! modals) and one function per message id of the design's message catalog
//! (§12), shared by Discord replies, DMs, the log channel, the dashboard,
//! and API error messages, so the same fact reads the same way everywhere.
//! `quack-discord` renders a [`Msg`] to Components V2; everything else uses
//! the plain-text renderer. Text people typed is always escaped.
//!
//! Functions take typed facts (core states and small view structs defined
//! here), never strings for states, and every state, outcome, failure, and
//! refusal is matched exhaustively, so a new variant does not compile until
//! it has words. Every message is snapshot-tested.

pub mod access;
pub mod action;
pub mod actions;
pub mod activity;
pub mod api;
pub mod appeal;
pub mod budget;
pub mod card;
pub mod case;
pub mod command;
pub mod dm;
pub mod error;
pub mod evidence;
pub mod fmt;
pub mod help;
pub mod honeypot;
pub mod log;
pub mod model;
pub mod ops;
pub mod render;
pub mod rules;
pub mod setup;
pub mod ticket;
pub mod timeline;
pub mod word;

pub use actions::{ButtonAction, ModalAction, SelectAction};
pub use model::{
    Actor, Block, Button, ChannelRef, Command, Inline, Modal, Msg, Person, RoleRef, Row, Select,
    Text, TimeStyle, Tone,
};
pub use render::{CommandIds, Surface};

#[cfg(test)]
mod tests;
