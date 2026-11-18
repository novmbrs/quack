//! `/case add member rule [private]` and its rule autocomplete (§11.1,
//! §11.6).

use quack_core::DomainError;
use quack_core::ids::{CaseId, RuleId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{AutocompleteFor, Person, RuleArg};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::cache::{CachedRule, GuildRules};
use quack_engine::cases::{Actor, ApplyRule, apply_rule};
use quack_engine::drafts::{CommandDraft, page_fields, pages, start};
use quack_text::actions::DraftPage;
use quack_text::case::{FormField, form_modal, picker_fill_form};
use quack_text::error::ErrorFacts;
use quack_text::fmt;

use super::outcome::{self, receipt};
use super::{op_key, person};
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;

/// Most characters in an autocomplete label or value.
const CHOICE_MAX: usize = 100;

/// Rule autocomplete (§11.1): from the rules cache only, and nothing for
/// someone the payload shows without `moderate`.
pub(crate) async fn autocomplete(p: Pending<'_>, command: AutocompleteFor, query: &str) -> Finish {
    let allowed = payload_capabilities(p.ix, p.config().as_deref()).moderate;
    let choices: Vec<AutocompleteChoice> = if allowed {
        p.rule_choices(query, command == AutocompleteFor::CaseList)
            .into_iter()
            .take(25)
            .map(|c| AutocompleteChoice {
                name: fmt::cut(&c.name, CHOICE_MAX),
                value: c.rule.get().to_string(),
            })
            .collect()
    } else {
        Vec::new()
    };
    p.choices(&choices).await
}

/// The active rule a `rule` option names: a picked id, or typed text
/// matched exactly by name (§11.1).
pub(crate) fn find<'r>(rules: &'r GuildRules, arg: &RuleArg) -> Option<&'r CachedRule> {
    match arg {
        RuleArg::Id(id) => rules.active(*id),
        RuleArg::Name(name) => rules.active_by_name(name),
    }
}

/// The form fields of one page, empty or with saved answers.
pub(crate) fn form_fields(rule: &CachedRule, page: u8) -> Vec<FormField> {
    page_fields(&rule.definition, page)
        .into_iter()
        .map(|f| FormField {
            key: f.key,
            label: f.label,
            kind: f.kind,
            required: f.required,
            value: None,
        })
        .collect()
}

/// A form's page `page` for `rule`.
pub(crate) fn page_at(draft: CaseId, rule: &CachedRule, page: u8) -> DraftPage {
    DraftPage {
        draft,
        rule: rule.id,
        version: rule.version,
        page,
    }
}

/// `/case add`: when the cached rule has a required field and the payload
/// shows `moderate`, the first form page opens at once; otherwise the
/// command defers and applies the rule with the interaction id as the
/// operation key (§11.3, §11.6).
pub(crate) async fn run(
    p: Pending<'_>,
    member: &Person,
    rule: &RuleArg,
    private: Option<bool>,
) -> Finish {
    let private = private.unwrap_or(false);
    let cached = p.rules().and_then(|r| find(&r, rule).cloned());
    let moderator = payload_capabilities(p.ix, p.config().as_deref()).moderate;
    if let (Some(rule), true) = (&cached, moderator)
        && rule.definition.context_fields().iter().any(|f| f.required)
    {
        return open_form(p, member, rule, private).await;
    }
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = apply_now(&a, member, rule, private).await;
    a.finish_opt(result).await
}

/// Opens the first page of a fresh form (no stored values, §11.3), then,
/// after that acknowledgement, writes its draft row so the page's submit
/// finds the member and the receipt choice even after a restart.
async fn open_form(p: Pending<'_>, member: &Person, rule: &CachedRule, private: bool) -> Finish {
    let draft = uuid::Uuid::now_v7();
    let at = page_at(CaseId::from_uuid(draft), rule, 1);
    let modal = form_modal(
        &rule.definition.name,
        at,
        pages(&rule.definition),
        &form_fields(rule, 1),
    );
    let (ix, shared) = (p.ix, p.shared());
    let finish = p.modal(&modal).await;
    if finish == Finish::Done {
        let started = async {
            let principal = auth::principal(&shared.engine, ix).await?;
            let channel = ix.channel_id.ok_or_else(crate::router::expired)?;
            let form = CommandDraft {
                draft,
                member: member.account.id.member(),
                rule: Some((rule.id, rule.version)),
                source: None,
                private,
                channel,
            };
            start(&shared.engine, &principal, &form)
                .await
                .map_err(|e| outcome::apply_failed(&e, None, None))
        };
        if let Err(failure) = started.await {
            // The page's submit then answers that the form expired.
            tracing::warn!(?failure, "couldn't save a /case add form's draft");
        }
    }
    finish
}

/// The rule's id: picked, or found by its exact name.
async fn rule_id(
    a: &Acked<'_>,
    rule: &RuleArg,
) -> Result<(RuleId, Option<CachedRule>, Option<String>), Failure> {
    let engine = a.engine();
    let guild = auth::guild_of(a.ix)?;
    let rules = engine
        .rules_cache()
        .get(engine.db(), guild)
        .await
        .map_err(|e| errors::db(&e))?;
    match (find(&rules, rule), rule) {
        (Some(found), _) => Ok((
            found.id,
            Some(found.clone()),
            Some(found.definition.name.clone()),
        )),
        (None, RuleArg::Id(id)) => {
            let archived = rules
                .archived
                .iter()
                .find(|r| r.id == *id)
                .map(|r| r.name.clone());
            Ok((*id, None, archived))
        }
        (None, RuleArg::Name(_)) => Err(errors::domain(
            DomainError::RuleNotFound,
            &ErrorFacts::default(),
        )),
    }
}

/// Applies the rule now, without case context. A rule whose required
/// fields the cache didn't know about answers with **Fill in the form**.
async fn apply_now(
    a: &Acked<'_>,
    member: &Person,
    rule: &RuleArg,
    private: bool,
) -> Result<Option<quack_text::Msg>, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    // Before the rule lookup, so a refusal never tells which names exist.
    auth::require(engine, &principal, Capability::Moderate).await?;
    let (rule, cached, name) = rule_id(a, rule).await?;
    if let Some(found) = cached.as_ref()
        && found.definition.context_fields().iter().any(|f| f.required)
    {
        let draft = uuid::Uuid::now_v7();
        let channel = a.ix.channel_id.ok_or_else(crate::router::expired)?;
        let form = CommandDraft {
            draft,
            member: member.account.id.member(),
            rule: Some((found.id, found.version)),
            source: None,
            private,
            channel,
        };
        start(engine, &principal, &form)
            .await
            .map_err(|e| outcome::apply_failed(&e, None, None))?;
        let first = page_at(CaseId::from_uuid(draft), found, 1);
        return Ok(Some(picker_fill_form(
            &found.definition.name,
            &person(member),
            first,
        )));
    }
    let channel = a.ix.channel_id.ok_or_else(crate::router::expired)?;
    let request = ApplyRule {
        guild: principal.guild,
        actor: Actor::Staff(principal.clone()),
        member: member.account.id.member(),
        rule,
        pinned_version: None,
        context: Vec::new(),
        evidence_links: Vec::new(),
        source_message: None,
        receipt: Some(receipt(a.ix, private, channel)),
        replaces: None,
        op_key: op_key(a.ix, "case_add"),
        draft: None,
        honeypot: None,
    };
    let member_text = person(member);
    match apply_rule(engine, request).await {
        Ok(applied) => outcome::applied(engine, &principal, &applied).await,
        Err(e) => Err(errors::apply(
            &e,
            &ErrorFacts {
                scene: quack_text::error::Scene::AddingCase,
                rule: name.as_deref(),
                member: Some(&member_text),
                ..ErrorFacts::default()
            },
        )),
    }
}
