//! The member timeline (§4.11): built from immutable rows, so history
//! never changes when an action is retried or dismissed. Member-facing
//! words that never name staff.

use quack_core::action::{ActionState, AttemptOutcome, Failure};
use quack_core::appeal::AppealEventKind;
use quack_core::modlog::ModLogEntry;
use quack_db::modlog::StoredEntry;
use quack_text::render::plain;
use quack_text::{Actor, Person};

use super::records::CaseRecords;
use super::view::TimelineText;

/// Most member timeline items returned (the newest).
const LIMIT: usize = 100;

/// The member timeline, oldest first, and whether older items were cut.
/// `log` is the case's moderation log, whose `action_failed` entries are
/// each action's failures.
pub(crate) fn member_timeline(r: &CaseRecords, log: &[StoredEntry]) -> (Vec<TimelineText>, bool) {
    let case = &r.case;
    let mut items = vec![TimelineText {
        at: case.created_at,
        text: plain(&quack_text::timeline::case_added(case.number)),
    }];
    for attempt in &r.attempts {
        let (Some(at), Some(AttemptOutcome::Done | AttemptOutcome::NoChange)) =
            (attempt.finished_at, attempt.outcome)
        else {
            continue;
        };
        let Some(action) = r.actions.iter().find(|a| a.id == attempt.action) else {
            continue;
        };
        if action.state == ActionState::Skipped {
            continue;
        }
        let mut view = r.action_view(action);
        view.state = ActionState::Done;
        view.no_change = attempt.outcome == Some(AttemptOutcome::NoChange);
        items.push(TimelineText {
            at,
            text: plain(&quack_text::action::member_sentence(&view)),
        });
    }
    for entry in log {
        let ModLogEntry::ActionFailed(detail) = &entry.entry else {
            continue;
        };
        let Some(action) = r
            .actions
            .iter()
            .find(|a| Some(a.id) == entry.links.action_id)
        else {
            continue;
        };
        let mut view = r.action_view(action);
        view.state = ActionState::Failed;
        view.failure = Some(Failure {
            reason: detail.reason,
            permission: detail.permission,
            http_status: detail.http_status,
            discord_code: detail.discord_code,
        });
        items.push(TimelineText {
            at: entry.created_at,
            text: plain(&quack_text::action::member_sentence(&view)),
        });
    }
    for action in r.actions.iter().filter(|a| a.state == ActionState::Skipped) {
        // When it was skipped (its log entry), not when it was requested.
        let at = log
            .iter()
            .find(|e| {
                matches!(e.entry, ModLogEntry::ActionUndoSkipped(_))
                    && e.links.action_id == Some(action.id)
            })
            .map_or(action.created_at, |e| e.created_at);
        items.push(TimelineText {
            at,
            text: plain(&quack_text::action::member_sentence(&r.action_view(action))),
        });
    }
    if let Some(dm) = &r.dm
        && let Some(at) = dm.settled_at
    {
        items.push(TimelineText {
            at,
            text: plain(&quack_text::dm::member_sentence(r.dm_view().as_ref())),
        });
    }
    let you = Person::unnamed(case.member.account());
    for event in &r.appeal_events {
        let actor = match (event.kind, event.closed_by_void) {
            (AppealEventKind::Closed, true) => Some(Actor::Quack),
            _ => event.actor.map(|a| Actor::Person(Person::unnamed(a))),
        };
        items.push(TimelineText {
            at: event.created_at,
            text: plain(&quack_text::appeal::event(
                event.kind,
                actor.as_ref(),
                &you,
                true,
            )),
        });
    }
    if let Some(void) = &case.void {
        items.push(TimelineText {
            at: void.at,
            text: plain(&quack_text::timeline::case_voided(&void.reason)),
        });
    }
    items.sort_by_key(|i| i.at);
    let truncated = items.len() > LIMIT;
    let skip = items.len().saturating_sub(LIMIT);
    (items.into_iter().skip(skip).collect(), truncated)
}
