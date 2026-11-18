//! The member's side of an appeal (§8.1, §8.2): submitting it, and
//! replying when staff asked for more information.

use quack_core::DomainError;
use quack_core::appeal::{
    AppealActor, AppealEventKind, EligibilityFacts, eligibility, statement_ok, transition,
};
use quack_core::ids::{AccountId, AppealId, CaseId, MessageRowId};
use quack_db::appeals;
use quack_db::messages::{self, NewTrackedMessage, Owner};
use quack_db::{cases, rules, settings};
use quack_telemetry::metrics;
use serde::Serialize;

use super::{AppealError, Stored, event, log, row};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash, stored_id};
use crate::{Engine, OpKey, Origin, obs};

/// What [`submit`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submitted {
    /// The appeal.
    pub appeal: AppealId,
    /// Its case.
    pub case: CaseId,
    /// The same request already ran.
    pub replayed: bool,
}

/// What [`add_information`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Replied {
    /// The appeal.
    pub appeal: AppealId,
    /// Its new version.
    pub version: u32,
    /// The same request already ran.
    pub replayed: bool,
}

#[derive(Serialize)]
struct SubmitRequest<'a> {
    case: CaseId,
    statement: &'a str,
}

#[derive(Serialize)]
struct ReplyRequest<'a> {
    case: CaseId,
    text: &'a str,
    expected_version: u32,
}

/// Submits the member's appeal of their own case (§8.1). The submit locks
/// the case first, so it and a void are ordered: an appeal is never
/// created on a voided case. Someone else's case reads as not found.
pub async fn submit(
    engine: &Engine,
    member: AccountId,
    origin: Origin,
    case: CaseId,
    statement: &str,
    op_key: &OpKey,
) -> Result<Submitted, AppealError> {
    obs::use_case("submit_appeal", None, async {
        if !statement_ok(statement) {
            return Err(AppealError::TextLength);
        }
        let found = {
            let mut conn = engine.db().conn().await?;
            cases::get_for_member(&mut conn, member.member(), case)
                .await?
                .ok_or(DomainError::CaseNotFound)?
        };
        let guild = found.guild;
        let key = op_key.scoped(None, Some(member));
        let hash = request_hash(&SubmitRequest { case, statement });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                let s: Stored = serde_json::from_value(response)
                    .map_err(|e| quack_db::DbError::Decode(e.to_string()))?;
                return Ok(Submitted {
                    appeal: s.appeal_id,
                    case: s.case_id,
                    replayed: true,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        // The settings row first (shared): a change of the appeal channel
        // in flight finishes before this reads it, and one that starts later
        // waits for this appeal and then moves its post (§4.12, §8.3).
        let channel = settings::lock_shared(&mut tx, guild)
            .await?
            .and_then(|s| s.appeal_channel);
        let locked = cases::lock(&mut tx, guild, case)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let existing = appeals::lock_for_case(&mut tx, guild, case).await?;
        let appealable = match locked.rule {
            Some((rule, version)) => rules::version(&mut tx, guild, rule, version)
                .await?
                .is_some_and(|v| v.definition.appealable),
            None => false,
        };
        eligibility(&EligibilityFacts {
            is_member: locked.member == member.member(),
            case_valid: locked.is_valid(),
            appealable,
            has_appeal: existing.is_some(),
            source: locked.source,
        })?;
        let id = create(&mut tx, &locked, member, origin, statement, channel).await?;
        messages::bump(&mut tx, guild, Owner::Case(case)).await?;
        let stored = serde_json::to_value(Stored {
            appeal_id: id,
            case_id: case,
        })
        .unwrap_or_default();
        complete(&mut tx, &key, 201, stored).await?;
        tx.commit().await?;
        metrics()
            .appeals
            .transition(obs::appeal_transition(AppealEventKind::Submitted));
        Ok(Submitted {
            appeal: id,
            case,
            replayed: false,
        })
    })
    .await
}

/// The appeal's rows (§8.2 `submitted`): the appeal, its event, its staff
/// post (in `channel`, the appeal channel read under the settings lock, or
/// waiting for one), and the log entry.
async fn create(
    tx: &mut quack_db::Tx,
    case: &quack_db::cases::Case,
    member: AccountId,
    origin: Origin,
    statement: &str,
    channel: Option<quack_core::ids::ChannelId>,
) -> Result<AppealId, AppealError> {
    let guild = case.guild;
    let id = AppealId::from_uuid(uuid::Uuid::now_v7());
    appeals::insert(tx, id, guild, case.id, case.member, statement.trim())
        .await
        .map_err(|e| match e.conflict() {
            Some(quack_db::Conflict::AppealExists) => {
                AppealError::Refused(DomainError::AppealExists)
            }
            _ => e.into(),
        })?;
    let appeal = appeals::get(tx, guild, id)
        .await?
        .ok_or(quack_db::DbError::NotFound)?;
    event(tx, &appeal, AppealEventKind::Submitted, member, statement).await?;
    messages::insert(
        tx,
        &NewTrackedMessage {
            id: MessageRowId::from_uuid(uuid::Uuid::now_v7()),
            guild,
            owner: Owner::Appeal(id),
            channel,
            private: false,
            interaction_token: None,
        },
    )
    .await?;
    log(
        tx,
        case,
        id,
        AppealEventKind::Submitted,
        member,
        origin.mod_log_source(),
    )
    .await?;
    Ok(id)
}

/// The member replies with the information staff asked for, naming the
/// appeal `version` they saw, so an old reply never answers a newer
/// request (§8.2).
pub async fn add_information(
    engine: &Engine,
    member: AccountId,
    origin: Origin,
    case: CaseId,
    text: &str,
    expected_version: u32,
    op_key: &OpKey,
) -> Result<Replied, AppealError> {
    obs::use_case("add_appeal_information", None, async {
        if !statement_ok(text) {
            return Err(AppealError::TextLength);
        }
        let found = {
            let mut conn = engine.db().conn().await?;
            cases::get_for_member(&mut conn, member.member(), case)
                .await?
                .ok_or(DomainError::CaseNotFound)?
        };
        let guild = found.guild;
        let key = op_key.scoped(None, Some(member));
        let hash = request_hash(&ReplyRequest {
            case,
            text,
            expected_version,
        });
        let mut tx = match begin_keyed(engine.db(), &key, &hash).await? {
            Keyed::Replay(response) => {
                return Ok(Replied {
                    appeal: stored_id(&response, "appeal_id")?,
                    version: stored_id(&response, "version")?,
                    replayed: true,
                });
            }
            Keyed::Fresh(tx) => tx,
        };
        // The settings row first (shared): a change of the appeal channel
        // in flight finishes before this reads it, and one that starts later
        // waits for this appeal and then moves its post (§4.12, §8.3).
        let channel = settings::lock_shared(&mut tx, guild)
            .await?
            .and_then(|s| s.appeal_channel);
        let locked = cases::lock(&mut tx, guild, case)
            .await?
            .ok_or(DomainError::CaseNotFound)?;
        let appeal = appeals::lock_for_case(&mut tx, guild, case)
            .await?
            .ok_or(DomainError::AppealNotFound)?;
        let current = row(&mut tx, &appeal).await?;
        let change = transition(
            &current,
            locked.is_valid(),
            AppealEventKind::InformationProvided,
            AppealActor::Member,
            expected_version,
        )?;
        let version = appeals::transition(
            &mut tx,
            guild,
            appeal.id,
            appeal.state,
            Some(expected_version),
            change.to,
            None,
        )
        .await?
        .ok_or(DomainError::AppealChanged)?;
        event(
            &mut tx,
            &appeal,
            AppealEventKind::InformationProvided,
            member,
            text,
        )
        .await?;
        messages::bump(&mut tx, guild, Owner::Appeal(appeal.id)).await?;
        log(
            &mut tx,
            &locked,
            appeal.id,
            AppealEventKind::InformationProvided,
            member,
            origin.mod_log_source(),
        )
        .await?;
        let stored = serde_json::json!({ "appeal_id": appeal.id, "version": version });
        complete(&mut tx, &key, 200, stored).await?;
        tx.commit().await?;
        metrics()
            .appeals
            .transition(obs::appeal_transition(AppealEventKind::InformationProvided));
        Ok(Replied {
            appeal: appeal.id,
            version,
            replayed: false,
        })
    })
    .await
}
