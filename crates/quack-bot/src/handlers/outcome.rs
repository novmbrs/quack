//! What adding a case answers (§11.6, §11.7), shared by `/case add`, the
//! "Add case" commands, and the draft pages.
//!
//! A new case's receipt is a tracked message: its sync job is the only
//! writer of the interaction's original response (the private receipt, or
//! `case.receipt.posted` / `case.receipt.post_failed` next to a public
//! receipt). So a new case answers nothing more here; every other outcome
//! answers in the original response.

use quack_core::ids::{CaseId, ChannelId};
use quack_discord::interaction::Interaction;
use quack_engine::cases::{Applied, ApplyError, Receipt};
use quack_engine::drafts::PageSaved;
use quack_engine::port::InteractionToken;
use quack_engine::{CaseRef, Engine, Principal};
use quack_text::actions::DraftPage;
use quack_text::case::{self, AlreadySaved};
use quack_text::error::{ErrorFacts, Scene};
use quack_text::{Msg, Tone};
use time::{Duration, OffsetDateTime};

use crate::ack::Failure;
use crate::errors;

/// How long before the 15-minute token lifetime Quack stops using it.
const TOKEN_MARGIN: Duration = Duration::seconds(60);

/// When the interaction's token stops being used (15 minutes after the
/// interaction, minus a minute).
pub(crate) fn token_expiry(ix: &Interaction) -> OffsetDateTime {
    ix.created_at().unwrap_or_else(OffsetDateTime::now_utc) + Duration::minutes(15) - TOKEN_MARGIN
}

/// Where this interaction's case receipt goes (§11.7).
pub(crate) fn receipt(ix: &Interaction, private: bool, channel: ChannelId) -> Receipt {
    let token = InteractionToken::new(ix.token.expose());
    let expires_at = token_expiry(ix);
    if private {
        Receipt::Private {
            channel,
            token,
            expires_at,
        }
    } else {
        Receipt::Public {
            channel,
            token: Some((token, expires_at)),
        }
    }
}

/// The answer for an applied rule; `None` when the receipt's sync answers.
pub(crate) async fn applied(
    engine: &Engine,
    principal: &Principal,
    applied: &Applied,
) -> Result<Option<Msg>, Failure> {
    match applied {
        Applied::Created(_) => Ok(None),
        Applied::Replayed(case) => {
            let view = quack_engine::cases::staff_card(engine, principal, CaseRef::Id(*case))
                .await
                .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
            Ok(Some(Msg::text(
                Tone::Neutral,
                case::receipt_replayed(view.number),
            )))
        }
    }
}

/// The answer when applying failed: a failure that may be repeated safely
/// offers **Try again** (`cs`) for a draft (§11.6 step 6).
pub(crate) fn apply_failed(err: &ApplyError, rule: Option<&str>, draft: Option<CaseId>) -> Failure {
    let facts = ErrorFacts {
        scene: Scene::AddingCase,
        rule,
        ..ErrorFacts::default()
    };
    let failure = errors::apply(err, &facts);
    match draft {
        Some(draft) if retryable(err) => failure.with_retry(draft),
        _ => failure,
    }
}

/// Whether a failed apply left the completed draft for **Try again**.
pub(crate) const fn retryable(err: &ApplyError) -> bool {
    matches!(
        err,
        ApplyError::Unconfirmed
            | ApplyError::DiscordUnavailable
            | ApplyError::Deadline
            | ApplyError::Db(_)
    )
}

/// The answer for a saved draft page or a picked rule (§11.6).
pub(crate) async fn page_saved(
    engine: &Engine,
    principal: &Principal,
    at: DraftPage,
    rule: &str,
    saved: PageSaved,
) -> Result<Option<Msg>, Failure> {
    let next = |page: u8| DraftPage { page, ..at };
    Ok(Some(match saved {
        PageSaved::Next { page, pages } => case::draft_page_saved(page - 1, pages, next(page)),
        PageSaved::Invalid(problems) => {
            let fields = engine
                .rules_cache()
                .cached(principal.guild)
                .and_then(|r| {
                    r.active(at.rule)
                        .map(|c| c.definition.context_fields().into_owned())
                })
                .unwrap_or_default();
            let lines = errors::context_problems(&problems, &fields);
            case::draft_page_invalid(&lines, at)
        }
        PageSaved::AlreadySaved { page } => {
            case::draft_page_already_saved(AlreadySaved::Continue(next(page)))
        }
        PageSaved::Expired => Msg::text(Tone::Neutral, case::draft_expired()),
        PageSaved::RuleChanged => Msg::text(Tone::Warning, case::draft_rule_changed(rule)),
        PageSaved::Applied(Applied::Replayed(case)) => {
            case::draft_page_already_saved(AlreadySaved::Case(case))
        }
        PageSaved::Applied(created @ Applied::Created(_)) => {
            return applied(engine, principal, &created).await;
        }
    }))
}
