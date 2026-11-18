//! Every sentence a person reads from Quack.
//!
//! A small neutral message model ([`Msg`], [`Text`], buttons, selects,
//! modals) shared by Discord replies, DMs, the log channel, the dashboard,
//! and API error messages. `quack-discord` renders a [`Msg`] to Components
//! V2; everything else uses the plain-text renderer. Text people typed is
//! always escaped. Discord's limits live in [`budget`].

pub mod actions;
pub mod budget;
pub mod command;
pub mod fmt;
pub mod model;
pub mod render;
pub mod word;

pub use actions::{ButtonAction, ModalAction, SelectAction};
pub use model::{
    Actor, Block, Button, ChannelRef, Command, Inline, Modal, Msg, Person, RoleRef, Row, Select,
    Text, TimeStyle, Tone,
};
pub use render::{CommandIds, Surface};
