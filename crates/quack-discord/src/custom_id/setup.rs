//! `/setup` area and operation codes (§11.8).

use quack_text::actions::{SetupArea, SetupOp};

pub(super) const fn area_code(area: SetupArea) -> &'static str {
    match area {
        SetupArea::Home => "home",
        SetupArea::Log => "log",
        SetupArea::Appeals => "ap",
        SetupArea::Evidence => "ev",
        SetupArea::Tickets => "tk",
        SetupArea::Honeypot => "hp",
        SetupArea::ActivityLog => "al",
    }
}

pub(super) fn area(code: &str) -> Option<SetupArea> {
    Some(match code {
        "home" => SetupArea::Home,
        "log" => SetupArea::Log,
        "ap" => SetupArea::Appeals,
        "ev" => SetupArea::Evidence,
        "tk" => SetupArea::Tickets,
        "hp" => SetupArea::Honeypot,
        "al" => SetupArea::ActivityLog,
        _ => return None,
    })
}

/// `open` shows an area; the design names every other code.
pub(super) const fn op_code(op: SetupOp) -> &'static str {
    match op {
        SetupOp::Open => "open",
        SetupOp::Channel => "ch",
        SetupOp::Create => "new",
        SetupOp::Clear => "clr",
        SetupOp::SetInvite => "inv",
        SetupOp::ClearInvite => "clrinv",
        SetupOp::TicketEntry => "entry",
        SetupOp::TicketQueue => "queue",
        SetupOp::Rule => "rule",
        SetupOp::ClearRule => "clrrule",
        SetupOp::EditWarning => "warn",
        SetupOp::TurnOn => "on",
        SetupOp::TurnOff => "off",
    }
}

pub(super) fn op(code: &str) -> Option<SetupOp> {
    Some(match code {
        "open" => SetupOp::Open,
        "ch" => SetupOp::Channel,
        "new" => SetupOp::Create,
        "clr" => SetupOp::Clear,
        "inv" => SetupOp::SetInvite,
        "clrinv" => SetupOp::ClearInvite,
        "entry" => SetupOp::TicketEntry,
        "queue" => SetupOp::TicketQueue,
        "rule" => SetupOp::Rule,
        "clrrule" => SetupOp::ClearRule,
        "warn" => SetupOp::EditWarning,
        "on" => SetupOp::TurnOn,
        "off" => SetupOp::TurnOff,
        _ => return None,
    })
}

/// Every area, for tests.
#[cfg(test)]
pub(super) const AREAS: [SetupArea; 7] = [
    SetupArea::Home,
    SetupArea::Log,
    SetupArea::Appeals,
    SetupArea::Evidence,
    SetupArea::Tickets,
    SetupArea::Honeypot,
    SetupArea::ActivityLog,
];

/// Every operation, for tests.
#[cfg(test)]
pub(super) const OPS: [SetupOp; 13] = [
    SetupOp::Open,
    SetupOp::Channel,
    SetupOp::Create,
    SetupOp::Clear,
    SetupOp::SetInvite,
    SetupOp::ClearInvite,
    SetupOp::TicketEntry,
    SetupOp::TicketQueue,
    SetupOp::Rule,
    SetupOp::ClearRule,
    SetupOp::EditWarning,
    SetupOp::TurnOn,
    SetupOp::TurnOff,
];
