//! One module per source table group. Each maps its Go rows for one server
//! (`import(cx)`) and reports what it skipped, changed, or derived.

pub(crate) mod actions;
pub(crate) mod appeal_notifications;
pub(crate) mod appeals;
pub(crate) mod audit;
pub(crate) mod cases;
pub(crate) mod counted;
pub(crate) mod evidence;
pub(crate) mod guilds;
pub(crate) mod honeypot;
pub(crate) mod modules;
pub(crate) mod notifications;
pub(crate) mod publications;
pub(crate) mod rules;
pub(crate) mod tickets;
