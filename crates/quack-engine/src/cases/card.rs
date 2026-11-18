//! The staff card and the public receipt (§11.7), built from a case's
//! records. `quack-text` turns them into words.

use quack_core::evidence::EvidenceState;
use quack_core::ids::AccountId;
use quack_text::case::{CardAction, CaseCard, EvidenceSummary, Imported, ReceiptView, Voided};
use quack_text::{Actor, Person};
use time::OffsetDateTime;

use super::records::CaseRecords;
use crate::Engine;
use crate::dms::context_lines;

fn person(id: AccountId) -> Person {
    Person::unnamed(id)
}

/// The staff card's facts.
pub(crate) fn card(engine: &Engine, r: &CaseRecords) -> CaseCard {
    let case = &r.case;
    let now = OffsetDateTime::now_utc();
    let card_action = |a: &quack_db::actions::Action| {
        let controls = r.controls(a, now);
        CardAction {
            id: a.id,
            view: r.action_view(a),
            can_retry: controls.can_retry(),
            can_dismiss: controls.can_dismiss(),
            can_undo: controls.can_undo(),
        }
    };
    let mut evidence = EvidenceSummary::default();
    for e in &r.evidence {
        match e.state {
            EvidenceState::Saving => evidence.saving += 1,
            EvidenceState::Saved => evidence.saved += 1,
            EvidenceState::Failed => evidence.failed += 1,
        }
    }
    let decision = case.decision;
    CaseCard {
        case: case.id,
        number: case.number,
        rule: r.rule_name(),
        member: person(case.member.account()),
        member_id: case.member,
        source: case.source,
        level_index: decision.map_or(0, |d| d.level_index),
        level_from: decision.map_or(0, |d| d.level_from_case_count),
        case_count: decision.map_or(0, |d| d.case_count),
        reason: r.reason(),
        action: r.original().map(card_action),
        undo: r.undo().map(card_action),
        dm: r.dm_view(),
        evidence,
        context: r
            .definition
            .as_ref()
            .map_or_default(|d| context_lines(d, &case.context)),
        voided: case.void.as_ref().map(|v| Voided {
            by: v.by.map_or(Actor::Quack, |by| Actor::Person(person(by))),
            at: v.at,
            reason: v.reason.clone(),
        }),
        replaces: r.replaces.map(|c| c.number),
        replaced_by: r.replaced_by.map(|c| c.number),
        appeal: r.appeal.as_ref().map(|a| a.state),
        moderator: case.moderator.map(person),
        created_at: case.created_at,
        imported: case.imported.as_ref().map(|i| Imported {
            v4_action: json_text(i, "v4_action"),
            v4_moderator: json_text(i, "v4_moderator"),
        }),
        dashboard_url: engine.links().case(case.guild, case.number),
        failures_url: engine.links().failures(case.guild),
    }
}

/// The public receipt's facts: only its fixed fields (§11.7).
pub(crate) fn receipt(engine: &Engine, r: &CaseRecords) -> ReceiptView {
    card(engine, r).receipt()
}

fn json_text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
