//! Applying a rule (§4.2): the steps before the transaction. The
//! transaction itself is in `apply_tx`.

use std::time::Instant;

use quack_core::DomainError;
use quack_core::case::{
    CaseSource, ContextInput, ContextValues, MessageLink, fill_message_links, validate_context,
};
use quack_core::ids::{CaseId, GuildId, MemberId, RuleId};
use quack_core::limits::MAX_EVIDENCE_ITEMS;
use quack_core::modlog::{CaseCreateRefusedDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::{
    ActorPresence, Capability, CheckPurpose, MemberRefusal, check_member,
};
use quack_core::rule::RuleDefinition;
use quack_db::operation_keys::OperationKey;
use quack_db::{CONTENTION_RETRIES, cases};
use quack_telemetry::metrics;
use serde::Serialize;

use super::facts::{GatheredFacts, gather};
use super::{Actor, Applied, ApplyError, ApplyRule, Receipt, apply_tx, parse_links};
use crate::Engine;
use crate::modlog::{self, By};
use crate::obs;
use crate::opkey::{Keyed, begin_keyed, request_hash, stored_id};

/// The resolved request a case's operation key protects (§4.3): its hash
/// is stored on the case forever.
#[derive(Serialize)]
struct RequestForHash<'a> {
    member: MemberId,
    rule: RuleId,
    pinned_version: Option<u32>,
    context: Vec<(&'a str, String)>,
    evidence_links: &'a [String],
    source_message: Option<String>,
    private: bool,
    replaces: Option<CaseId>,
    source: &'static str,
}

/// The context as a request hash sees it: sorted by field key (JSON
/// object order is immaterial), each answer tagged with its kind so a
/// number and the same digits as text differ. Answers to one key keep
/// their order, so a duplicate is still a different request.
pub(super) fn context_for_hash(context: &[(String, ContextInput)]) -> Vec<(&str, String)> {
    let mut pairs: Vec<(&str, String)> = context
        .iter()
        .map(|(k, v)| {
            let text = match v {
                ContextInput::Json(v) => format!("json:{v}"),
                ContextInput::JsonNumber(n) => format!("number:{n}"),
                ContextInput::Text(t) => format!("text:{t}"),
            };
            (k.as_str(), text)
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0));
    pairs
}

/// What the steps before the transaction settled.
pub(super) struct Prepared {
    pub source: CaseSource,
    pub key: OperationKey,
    pub hash: [u8; 32],
    pub early_version: Option<(u32, ContextValues)>,
    pub facts: GatheredFacts,
}

/// Applies a rule to a member: the only way to create a case (§4.2).
///
/// Nothing reaches Discord before the commit; the case's action, DM,
/// evidence, receipt, and log post are jobs the commit wakes.
pub async fn apply_rule(engine: &Engine, request: ApplyRule) -> Result<Applied, ApplyError> {
    let guild = request.guild;
    obs::use_case("apply_rule", Some(guild), async {
        let started = Instant::now();
        let source = match &request.actor {
            Actor::Staff(p) => p.origin.case_source(),
            Actor::Quack => CaseSource::Honeypot,
        };
        let result = apply(engine, &request, source).await;
        match &result {
            Ok(Applied::Created(created)) => {
                let d = created.case.decision;
                metrics().cases.created(
                    obs::case_source(source),
                    d.and_then(|d| d.action_kind()).map(obs::action_kind),
                    d.map_or(1, |d| u32::from(d.level_index)),
                    started.elapsed(),
                );
            }
            Err(ApplyError::Refused(error)) => {
                metrics()
                    .cases
                    .refused(obs::case_source(source), obs::refusal_reason(*error));
            }
            Ok(Applied::Replayed(_)) | Err(_) => {}
        }
        result
    })
    .await
}

async fn apply(
    engine: &Engine,
    request: &ApplyRule,
    source: CaseSource,
) -> Result<Applied, ApplyError> {
    let guild = request.guild;
    // 1. Authorize. Staff act only in their own server: a principal of
    //    another server holds no capability here, whatever it holds there.
    let (account, presence) = match &request.actor {
        Actor::Staff(principal) if principal.guild != guild => {
            return Err(DomainError::MissingCapability(Capability::Moderate).into());
        }
        Actor::Staff(principal) => {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            (Some(principal.account), principal.presence())
        }
        Actor::Quack => (None, ActorPresence::Quack),
    };
    // 2. Replay check.
    let key = request.op_key.scoped(Some(guild), account);
    let hash = hash_request(request, source);
    if let Some(replayed) = replayed_case(engine, guild, &key, &hash).await? {
        return Ok(Applied::Replayed(replayed));
    }
    // 3. Early checks against the rules cache.
    let mut context = request.context.clone();
    let early = early_checks(engine, request, &mut context).await?;
    // 4. Evidence sources are collected in the transaction from the
    //    validated context; explicit links are checked here.
    if let Err(problems) = parse_links(guild, &request.evidence_links) {
        return Err(ApplyError::LinksInvalid(problems));
    }
    if let Some((_, values)) = &early {
        let def = cached_definition(engine, request).await?;
        let sources = evidence_sources(request, values, def.as_ref())?;
        if sources.len() > MAX_EVIDENCE_ITEMS as usize {
            return Err(DomainError::EvidenceLimitReached { count: 0 }.into());
        }
    }
    // 5. Facts.
    let facts = gather(engine.discord(), guild, request.member, presence)
        .await
        .map_err(|_| ApplyError::DiscordUnavailable)?;
    // 6. Member checks 1–8.
    let purpose = match request.actor {
        Actor::Staff(_) => CheckPurpose::ApplyRule,
        Actor::Quack => CheckPurpose::ApplyHoneypotRule,
    };
    if let Err(refusal) = check_member(&facts.apply, purpose) {
        log_refusal(engine, request, source, refusal).await?;
        return Err(DomainError::MemberRefused(refusal).into());
    }
    let prepared = Prepared {
        source,
        key,
        hash,
        early_version: early,
        facts,
    };
    // 7–22. The transaction, run again after a serialization failure or
    // deadlock (§4.12).
    let mut retries = 0;
    loop {
        // The deadline starts before the key's reservation, which can wait
        // on a concurrent request holding the same key; its errors go
        // through the same contention retry.
        let started = std::time::Instant::now();
        let reserved = tokio::time::timeout(
            apply_tx::DEADLINE,
            begin_keyed(engine.db(), &prepared.key, &prepared.hash),
        )
        .await;
        let result = match reserved {
            Err(_) => Err(ApplyError::Deadline),
            Ok(Err(error)) => Err(error.into()),
            Ok(Ok(Keyed::Replay(response))) => stored_id(&response, "case_id")
                .map(Applied::Replayed)
                .map_err(Into::into),
            Ok(Ok(Keyed::Fresh(tx))) => {
                apply_tx::run(engine, request, &prepared, tx, started).await
            }
        };
        match result {
            Err(ApplyError::Db(e)) if e.is_contention() && retries < CONTENTION_RETRIES => {
                retries += 1;
            }
            other => return other,
        }
    }
}

fn hash_request(request: &ApplyRule, source: CaseSource) -> [u8; 32] {
    request_hash(&RequestForHash {
        member: request.member,
        rule: request.rule,
        pinned_version: request.pinned_version,
        context: context_for_hash(&request.context),
        evidence_links: &request.evidence_links,
        source_message: request.source_message.map(|l| l.to_url()),
        private: matches!(request.receipt, Some(Receipt::Private { .. })),
        replaces: request.replaces,
        source: source.as_str(),
    })
}

/// §4.3: the operation key's row (24 hours), else the key stored on the
/// case itself (forever).
async fn replayed_case(
    engine: &Engine,
    guild: GuildId,
    key: &OperationKey,
    hash: &[u8; 32],
) -> Result<Option<CaseId>, ApplyError> {
    let mut conn = engine.db().conn().await?;
    if let Some(stored) = quack_db::operation_keys::lookup(&mut conn, key).await? {
        return if stored.request_hash == *hash {
            Ok(Some(stored_id(&stored.response, "case_id")?))
        } else {
            Err(ApplyError::OperationKeyReused)
        };
    }
    match cases::find_by_operation_key(&mut conn, guild, &key.case_scoped()).await? {
        Some((case, Some(stored))) if stored == *hash => Ok(Some(case)),
        Some(_) => Err(ApplyError::OperationKeyReused),
        None => Ok(None),
    }
}

/// Step 3: the rule exists and is active in the cache, and the context is
/// valid for the cached version (when that is the version to apply).
/// Returns the version validated and its values.
async fn early_checks(
    engine: &Engine,
    request: &ApplyRule,
    context: &mut Vec<(String, ContextInput)>,
) -> Result<Option<(u32, ContextValues)>, ApplyError> {
    let cache = engine.rules_cache();
    let mut rules = cache.get(engine.db(), request.guild).await?;
    if rules.active(request.rule).is_none() && !rules.is_archived(request.rule) {
        rules = cache.reload(engine.db(), request.guild).await?;
    }
    let Some(rule) = rules.active(request.rule) else {
        return Err(if rules.is_archived(request.rule) {
            DomainError::RuleArchived
        } else {
            DomainError::RuleNotFound
        }
        .into());
    };
    if request.pinned_version.is_some_and(|v| v != rule.version) {
        return Ok(None);
    }
    let fields = rule.definition.context_fields();
    if let Some(link) = &request.source_message {
        fill_message_links(&fields, context, link);
    }
    validate_context(&fields, request.guild, context)
        .map(|values| Some((rule.version, values)))
        .map_err(ApplyError::ContextInvalid)
}

async fn cached_definition(
    engine: &Engine,
    request: &ApplyRule,
) -> Result<Option<RuleDefinition>, ApplyError> {
    let rules = engine.rules_cache().get(engine.db(), request.guild).await?;
    Ok(rules.active(request.rule).map(|r| r.definition.clone()))
}

/// Step 4: the source message, every `message_link` answer and link inside
/// a text answer into this server, and explicit links, without repeats.
pub(super) fn evidence_sources(
    request: &ApplyRule,
    values: &ContextValues,
    def: Option<&RuleDefinition>,
) -> Result<Vec<MessageLink>, ApplyError> {
    let mut sources: Vec<MessageLink> = request.source_message.into_iter().collect();
    let mut push = |link: MessageLink| {
        if !sources.iter().any(|s| s.message_id == link.message_id) {
            sources.push(link);
        }
    };
    if let Some(def) = def {
        for link in values.evidence_links(&def.context_fields(), request.guild) {
            push(link);
        }
    }
    let explicit =
        parse_links(request.guild, &request.evidence_links).map_err(ApplyError::LinksInvalid)?;
    for link in explicit {
        push(link);
    }
    Ok(sources)
}

/// Step 6: a refusal caused by Quack's own access is written to the
/// moderation log in its own short transaction (`case_create_refused`).
/// Honeypot cases leave this to the module (`honeypot_case_failed`).
pub(crate) async fn log_refusal(
    engine: &Engine,
    request: &ApplyRule,
    source: CaseSource,
    refusal: MemberRefusal,
) -> Result<(), ApplyError> {
    let Some(detail) = CaseCreateRefusedDetail::from_refusal(refusal) else {
        return Ok(());
    };
    if source == CaseSource::Honeypot {
        return Ok(());
    }
    let by = match &request.actor {
        Actor::Staff(p) => By {
            actor: Some(p.account),
            source: p.origin.mod_log_source(),
        },
        Actor::Quack => By::QUACK,
    };
    let mut tx = engine.db().begin().await?;
    modlog::write(
        &mut tx,
        request.guild,
        ModLogEntry::CaseCreateRefused(detail),
        by,
        ModLogLinks {
            member_id: Some(request.member),
            rule_id: Some(request.rule),
            ..ModLogLinks::default()
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
