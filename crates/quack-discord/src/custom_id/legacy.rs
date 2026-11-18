//! Go-era custom ids that must keep working (§11.5): each supported shape
//! is decoded exactly and its ULID mapped with `ulid_to_uuid` (§19.3).

use quack_core::ids::{ActionId, AppealId, CaseId, MemberId, TicketId, ulid_to_uuid};

use super::{CustomId, Legacy};

pub(super) fn decode(raw: &str) -> Option<Legacy> {
    let parts: Vec<&str> = raw.split(':').collect();
    let [area, verb, "v1", arg] = parts.as_slice() else {
        return None;
    };
    let ulid = || ulid_to_uuid(arg).ok();
    Some(match (*area, *verb) {
        ("appeal", "submit") => Legacy::Mapped(CustomId::Appeal(CaseId::from_uuid(ulid()?))),
        ("appeal", "accept" | "reject") => {
            Legacy::Mapped(CustomId::OpenAppeal(AppealId::from_uuid(ulid()?)))
        }
        ("ticket", "open") if arg.is_empty() => Legacy::Mapped(CustomId::OpenTicket),
        ("ticket", "close") => Legacy::Mapped(CustomId::CloseTicket(TicketId::from_uuid(ulid()?))),
        ("ticket", "repair") => {
            Legacy::Mapped(CustomId::FixTicketAccess(TicketId::from_uuid(ulid()?)))
        }
        ("case", "view" | "evidence" | "edit_context" | "void") => {
            Legacy::Mapped(CustomId::OpenCase(CaseId::from_uuid(ulid()?)))
        }
        ("case", "retry" | "dismiss") => Legacy::CaseOfAction(ActionId::from_uuid(ulid()?)),
        ("case", "user_detail") if arg.bytes().all(|b| b.is_ascii_digit()) => {
            let member: MemberId = arg.parse().ok()?;
            Legacy::Mapped(CustomId::History { member, page: 1 })
        }
        _ => return None,
    })
}
