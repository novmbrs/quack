//! The staff card (§11.7) from the engine's staff case view.
//!
//! The engine builds the card for tracked private receipts from its own
//! records, but that builder is crate-private; `/case view` and every
//! "Open case" therefore build the same `quack_text::case::CaseCard` here
//! from [`CaseView`], which carries the same facts.

use quack_core::action::{ActionKind, AttemptOutcome, Failure, may_have_applied};
use quack_core::evidence::EvidenceState;
use quack_core::ids::GuildId;
use quack_engine::Links;
use quack_engine::cases::{ActionItem, CaseView};
use quack_text::action::ActionView;
use quack_text::case::{CardAction, CaseCard, ContextLine, EvidenceSummary, Imported, Voided};
use quack_text::dm::DmView;
use quack_text::{Actor, Msg, Person};

/// The action's facts for its sentence.
fn action_view(item: &ActionItem, original: Option<&ActionItem>, case_number: u64) -> ActionView {
    let failure = item.failure_reason.map(|reason| Failure {
        reason,
        permission: (reason == quack_core::action::ActionFailure::QuackMissingPermission)
            .then(|| item.kind.permission()),
        http_status: item.http_status,
        discord_code: item.discord_code,
    });
    let attempts: Vec<Option<AttemptOutcome>> =
        item.attempt_log.iter().map(|a| a.outcome).collect();
    ActionView {
        kind: item.kind,
        state: item.state,
        failure,
        skip: item.skip_reason,
        still_applies_case: item.skipped_because_case_number,
        no_change: false,
        attempts: item.attempts,
        next_attempt_at: item.next_attempt_at,
        timeout_ends_at: match item.kind {
            ActionKind::RemoveTimeout => original.and_then(|o| o.timeout_ends_at),
            _ => item.timeout_ends_at,
        },
        ban_delete_message_seconds: item.delete_message_seconds,
        case_number,
        dismissed: item.dismissed_at.map(|at| {
            let by = item.dismissed_by.map_or(Actor::UnknownModerator, |by| {
                Actor::Person(Person::unnamed(by))
            });
            (by, at)
        }),
        may_have_applied: may_have_applied(&attempts),
    }
}

fn card_action(item: &ActionItem, original: Option<&ActionItem>, case_number: u64) -> CardAction {
    CardAction {
        id: item.id,
        view: action_view(item, original, case_number),
        can_retry: item.can_retry,
        can_dismiss: item.can_dismiss,
        can_undo: item.can_undo,
    }
}

fn context(v: &CaseView) -> Vec<ContextLine> {
    let values = v.context.clone().with_kinds(&v.fields);
    v.fields
        .iter()
        .filter_map(|f| {
            values.get(&f.key).map(|value| ContextLine {
                label: f.label.clone(),
                kind: f.kind,
                value: value.clone(),
            })
        })
        .collect()
}

fn evidence(v: &CaseView) -> EvidenceSummary {
    let mut out = EvidenceSummary::default();
    for e in &v.evidence {
        match e.state {
            EvidenceState::Saving => out.saving += 1,
            EvidenceState::Saved => out.saved += 1,
            EvidenceState::Failed => out.failed += 1,
        }
    }
    out
}

fn text_of(value: Option<&serde_json::Value>, key: &str) -> String {
    value
        .and_then(|v| v.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The staff card's facts.
pub(crate) fn of(v: &CaseView, guild: GuildId, links: Links<'_>) -> CaseCard {
    let original = v.actions.iter().find(|a| a.undo_of_action_id.is_none());
    let undo = v.actions.iter().find(|a| a.undo_of_action_id.is_some());
    CaseCard {
        case: v.id,
        number: v.number,
        rule: v.rule_name.clone(),
        member: Person::unnamed(v.member_id.account()),
        member_id: v.member_id,
        source: v.source,
        level_index: v
            .level_index
            .map_or(0, |l| u8::try_from(l).unwrap_or(u8::MAX)),
        level_from: v.level_from_case_count.unwrap_or(0),
        case_count: v.case_count.unwrap_or(0),
        reason: v.reason.clone(),
        action: original.map(|a| card_action(a, None, v.number)),
        undo: undo.map(|u| card_action(u, original, v.number)),
        dm: v.dm.as_ref().map(|d| DmView {
            state: d.state,
            failure: d.failure,
        }),
        evidence: evidence(v),
        context: context(v),
        voided: v.void.as_ref().map(|void| Voided {
            by: void
                .by
                .map_or(Actor::Quack, |by| Actor::Person(Person::unnamed(by))),
            at: void.at,
            reason: void.reason.clone(),
        }),
        replaces: v.replaces_case_number,
        replaced_by: v.replaced_by_case_number,
        appeal: v.appeal.as_ref().map(|a| a.state),
        moderator: v.moderator_id.map(Person::unnamed),
        created_at: v.created_at,
        imported: v.imported.as_ref().map(|i| Imported {
            v4_action: text_of(Some(i), "v4_action"),
            v4_moderator: text_of(Some(i), "v4_moderator"),
        }),
        dashboard_url: links.case(guild, v.number),
        failures_url: links.failures(guild),
    }
}

/// The staff card message.
pub(crate) fn msg(v: &CaseView, guild: GuildId, links: Links<'_>) -> Msg {
    quack_text::case::card(&of(v, guild, links))
}
