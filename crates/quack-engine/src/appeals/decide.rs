//! Staff decisions on an appeal (§8.2): request information, accept,
//! reject, close, reopen. Each needs a reason, names the version staff
//! saw, and sends the member one DM rendered from its own event.

use quack_core::DomainError;
use quack_core::appeal::{
    APPEAL_ACCEPTED_VOID_REASON, AppealActor, AppealDecision, decision_reason_ok, transition,
};
use quack_core::case::VoidCause;
use quack_core::ids::{AppealId, CaseId, DmId};
use quack_core::permissions::Capability;
use quack_db::appeals::{self, Decision};
use quack_db::cases;
use quack_db::dms::{self, DmOwner};
use quack_db::jobs::{self, NewJob};
use quack_db::messages::{self, Owner};
use quack_telemetry::metrics;
use serde::Serialize;
use time::OffsetDateTime;

use super::{AppealError, event, log, row};
use crate::cases::{VoidBy, void_in_tx, voided_metrics};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Principal, obs};

/// What [`decide`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decided {
    /// The appeal.
    pub appeal: AppealId,
    /// Its case.
    pub case: CaseId,
    /// Its new version.
    pub version: u32,
    /// The member's DM about this decision.
    pub dm: Option<DmId>,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
struct DecideRequest<'a> {
    appeal: AppealId,
    decision: &'static str,
    reason: &'a str,
    expected_version: u32,
}

/// The writes that go with a decision (§8.2 effects): accepting voids the
/// case; every decision bumps the receipt and the post, queues the member's
/// DM about this event, and is logged.
async fn effects(
    tx: &mut quack_db::Tx,
    case: &quack_db::cases::Case,
    appeal: AppealId,
    decision: AppealDecision,
    event_id: quack_core::ids::AppealEventId,
    actor: quack_core::ids::AccountId,
    source: quack_core::modlog::ModLogSource,
) -> Result<(Option<crate::cases::VoidEffects>, DmId), AppealError> {
    let guild = case.guild;
    let voided = if decision == AppealDecision::Accept {
        let by = VoidBy {
            actor,
            source,
            cause: VoidCause::AppealAccepted,
            reason: APPEAL_ACCEPTED_VOID_REASON,
            accepted_appeal: Some(appeal),
        };
        Some(void_in_tx(tx, case, by).await?)
    } else {
        messages::bump(tx, guild, Owner::Case(case.id)).await?;
        None
    };
    let dm = DmId::from_uuid(uuid::Uuid::now_v7());
    dms::insert(
        tx,
        dm,
        guild,
        case.member,
        DmOwner::AppealUpdate(case.id, event_id),
    )
    .await?;
    jobs::enqueue(tx, &NewJob::send_dm(guild, dm)).await?;
    messages::bump(tx, guild, Owner::Appeal(appeal)).await?;
    log(tx, case, appeal, decision.event_kind(), actor, source).await?;
    Ok((voided, dm))
}

/// The committed decision's metrics: the transition, the time to the first
/// accept, reject, or close, and the void an accept made.
fn record(
    appeal: &quack_db::appeals::Appeal,
    case: &quack_db::cases::Case,
    decision: AppealDecision,
    voided: Option<&crate::cases::VoidEffects>,
) {
    let m = &metrics().appeals;
    let label = obs::appeal_transition(decision.event_kind());
    m.transition(label);
    let decides = matches!(
        decision,
        AppealDecision::Accept | AppealDecision::Reject | AppealDecision::Close
    );
    if decides && appeal.first_decided_at.is_none() {
        m.decided(
            label,
            obs::since(appeal.created_at, OffsetDateTime::now_utc()),
        );
    }
    if let Some(effects) = voided {
        voided_metrics(case, VoidCause::AppealAccepted, effects);
    }
}

const fn name(decision: AppealDecision) -> &'static str {
    match decision {
        AppealDecision::RequestInformation => "request_information",
        AppealDecision::Accept => "accept",
        AppealDecision::Reject => "reject",
        AppealDecision::Close => "close",
        AppealDecision::Reopen => "reopen",
    }
}

/// A staff decision on an appeal (§8.2). Accepting voids the case in the
/// same transaction (cause `appeal_accepted`).
pub async fn decide(
    engine: &Engine,
    principal: &Principal,
    appeal: AppealId,
    decision: AppealDecision,
    reason: &str,
    expected_version: u32,
    op_key: &OpKey,
) -> Result<Decided, AppealError> {
    let guild = principal.guild;
    obs::use_case("decide_appeal", Some(guild), async {
        let found = {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            appeals::get(&mut conn, guild, appeal)
                .await?
                .ok_or(DomainError::AppealNotFound)?
        };
        if !decision_reason_ok(reason) {
            return Err(AppealError::ReasonLength);
        }
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&DecideRequest {
            appeal,
            decision: name(decision),
            reason,
            expected_version,
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                return Ok(Decided {
                    appeal,
                    case: found.case,
                    version: stored_id(&response, "version")?,
                    dm: None,
                    replayed: true,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        principal.require(&mut tx, Capability::Moderate).await??;
        let case = cases::lock(&mut tx, guild, found.case)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let locked = appeals::lock_for_case(&mut tx, guild, case.id)
            .await?
            .ok_or(DomainError::AppealNotFound)?;
        let kind = decision.event_kind();
        let change = transition(
            &row(&mut tx, &locked).await?,
            case.is_valid(),
            kind,
            AppealActor::Staff,
            expected_version,
        )?;
        let reason = reason.trim();
        let version = appeals::transition(
            &mut tx,
            guild,
            appeal,
            locked.state,
            Some(expected_version),
            change.to,
            Some(Decision {
                by: principal.account,
                reason,
            }),
        )
        .await?
        .ok_or(DomainError::AppealChanged)?;
        let event_id = event(&mut tx, &locked, kind, principal.account, reason).await?;
        let source = principal.origin.mod_log_source();
        let (voided, dm) = effects(
            &mut tx,
            &case,
            appeal,
            decision,
            event_id,
            principal.account,
            source,
        )
        .await?;
        complete(
            &mut tx,
            &key,
            200,
            serde_json::json!({ "version": version }),
        )
        .await?;
        tx.commit().await?;
        record(&locked, &case, decision, voided.as_ref());
        Ok(Decided {
            appeal,
            case: case.id,
            version,
            dm: Some(dm),
            replayed: false,
        })
    })
    .await
}
