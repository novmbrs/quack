//! Shared facts for samples.

use quack_core::ids::{
    AccountId, ActionId, AppealId, CaseId, ChannelId, MemberId, MessageId, RoleId, RuleId, TicketId,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::model::{Actor, ChannelRef, Command, Person, RoleRef, Text};
use crate::render::CommandIds;

pub fn t0() -> OffsetDateTime {
    time::macros::datetime!(2026-11-17 14:05:09 UTC)
}

pub fn later() -> OffsetDateTime {
    time::macros::datetime!(2026-11-18 15:05:09 UTC)
}

pub fn account(n: u64) -> AccountId {
    AccountId::new(n).unwrap()
}

pub fn member_id() -> MemberId {
    MemberId::new(222).unwrap()
}

pub fn sam() -> Person {
    Person::new(account(222), "Sam")
}

pub fn mod_ana() -> Person {
    Person::new(account(333), "Ana")
}

pub fn ana() -> Actor {
    Actor::Person(mod_ana())
}

pub fn channel(name: &str) -> ChannelRef {
    ChannelRef::new(ChannelId::new(444).unwrap(), name)
}

pub fn thread() -> ChannelRef {
    ChannelRef::new(ChannelId::new(555).unwrap(), "ticket-abc")
}

pub fn role() -> RoleRef {
    RoleRef {
        id: RoleId::new(666).unwrap(),
        name: Some("Helpers".to_owned()),
    }
}

pub fn message_id(n: u64) -> MessageId {
    MessageId::new(n).unwrap()
}

pub fn case_id() -> CaseId {
    CaseId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001))
}

pub fn rule_id() -> RuleId {
    RuleId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0002))
}

pub fn action_id() -> ActionId {
    ActionId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0003))
}

pub fn appeal_id() -> AppealId {
    AppealId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0004))
}

pub fn ticket_id() -> TicketId {
    TicketId::from_uuid(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0005))
}

pub const URL: &str = "https://quack.example/guilds/111/cases/12";

/// Hostile text a person might type: mentions, Markdown, links, invisible
/// characters, a heading, a list, and a quote.
pub const HOSTILE: &str = "@everyone <@1> <@&2> <#3> **bold** _it_ ~~x~~ ||spoil|| `code` [click](https://evil.example) https://evil.example\n# Heading\n> quote\n- list\n1. one\n\u{200B}hid\u{202E}den\u{2066}";

/// A maximal reason or answer.
pub fn long(n: usize) -> String {
    "Long text with *stars* and <@1> mentions. "
        .repeat(n / 40 + 1)
        .chars()
        .take(n)
        .collect()
}

pub fn commands() -> CommandIds {
    CommandIds(vec![
        (Command::CaseAdd, 1_001),
        (Command::CaseView, 1_001),
        (Command::CaseList, 1_001),
        (Command::CaseVoid, 1_001),
        (Command::CaseHistory, 1_001),
        (Command::Appeals, 1_002),
        (Command::Rules, 1_003),
        (Command::Setup, 1_004),
        (Command::Help, 1_005),
    ])
}

/// One problem line for messages that list problems.
pub fn problem(s: &str) -> Text {
    Text::fixed(s)
}
