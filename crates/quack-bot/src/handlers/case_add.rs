//! `/case add member rule [private] [file]` and its rule autocomplete
//! (§11.1, §11.6). A `file` becomes an uploaded-files item of the case,
//! refused before the case (or its draft) exists when Quack can't copy it.

use quack_core::DomainError;
use quack_core::ids::{CaseId, RuleId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{AutocompleteFor, Person, RuleArg, Upload};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::cache::{CachedRule, GuildRules};
use quack_engine::cases::{Actor, ApplyRule, apply_rule};
use quack_engine::drafts::{CommandDraft, page_fields, pages, start};
use quack_engine::evidence::{UploadRefused, check_destination, check_files};
use quack_engine::{Engine, Principal};
use quack_text::actions::DraftPage;
use quack_text::case::{FormField, form_modal, picker_fill_form};
use quack_text::error::ErrorFacts;
use quack_text::fmt;

use super::evidence::{draft_file, new_upload};
use super::outcome::{self, receipt};
use super::{op_key, person};
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;

/// Most characters in an autocomplete label or value.
const CHOICE_MAX: usize = 100;

/// How much of the prefilled-modal deadline (§11.3) the evidence channel
/// check may take before the draft is written; past it, the check is left
/// to applying the draft.
const DESTINATION_BUDGET: std::time::Duration = std::time::Duration::from_millis(150);

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
    file: Option<&Upload>,
) -> Finish {
    let private = private.unwrap_or(false);
    let cached = p.rules().and_then(|r| find(&r, rule).cloned());
    let moderator = payload_capabilities(p.ix, p.config().as_deref()).moderate;
    if let (Some(rule), true) = (&cached, moderator)
        && rule.definition.context_fields().iter().any(|f| f.required)
    {
        return open_form(p, member, rule, private, file).await;
    }
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = apply_now(&a, member, rule, private, file).await;
    a.finish_opt(result).await
}

/// Opens the first page of a fresh form after writing its draft row (the
/// member, the rule version, and the receipt choice), so the page's
/// submit always finds it, even after a restart. The write takes the
/// prefilled-modal exception (§11.3): bounded to 300 ms, else the
/// moderator is told Quack is busy and no form opens. A `file` Quack can't
/// copy is refused first, and the evidence channel is checked when that
/// fits the deadline (§11.3); applying the draft always checks it again.
async fn open_form(
    p: Pending<'_>,
    member: &Person,
    rule: &CachedRule,
    private: bool,
    file: Option<&Upload>,
) -> Finish {
    let draft = uuid::Uuid::now_v7();
    let at = page_at(CaseId::from_uuid(draft), rule, 1);
    let modal = form_modal(
        &rule.definition.name,
        at,
        pages(&rule.definition),
        &form_fields(rule, 1),
    );
    let ix = p.ix;
    p.prefilled_modal(|engine| async move {
        let principal = auth::principal(engine, ix).await?;
        let channel = ix.channel_id.ok_or_else(crate::router::expired)?;
        if let Some(file) = file {
            // Authorized first, so the channel check reveals nothing.
            auth::require(engine, &principal, Capability::Moderate).await?;
            destination(check_files(&[new_upload(file)]))?;
            if let Ok(checked) = tokio::time::timeout(
                DESTINATION_BUDGET,
                check_destination(engine, principal.guild),
            )
            .await
            {
                destination(checked)?;
            }
        }
        let form = CommandDraft {
            draft,
            member: member.account.id.member(),
            rule: Some((rule.id, rule.version)),
            source: None,
            private,
            channel,
            evidence_file: file.map(draft_file),
        };
        start(engine, &principal, &form)
            .await
            .map_err(|e| outcome::apply_failed(&e, None, None))?;
        Ok(modal)
    })
    .await
}

/// The evidence channel check's answer: only a channel that isn't ready
/// refuses; Discord not answering leaves it to the check when the case is
/// applied.
fn destination(checked: Result<(), UploadRefused>) -> Result<(), Failure> {
    match checked {
        Ok(()) | Err(UploadRefused::DiscordUnavailable) => Ok(()),
        Err(refused) => Err(errors::case(
            &quack_engine::cases::CaseError::from(refused),
            &ErrorFacts::default(),
        )),
    }
}

/// Starts a draft for the form with its file, which is refused first, in
/// §7.1's order, when Quack can't copy it or the evidence channel isn't
/// ready.
async fn start_form(
    engine: &Engine,
    principal: &Principal,
    form: &CommandDraft,
    file: Option<&Upload>,
) -> Result<(), Failure> {
    if let Some(file) = file {
        destination(check_files(&[new_upload(file)]))?;
        destination(check_destination(engine, principal.guild).await)?;
    }
    start(engine, principal, form)
        .await
        .map_err(|e| outcome::apply_failed(&e, None, None))
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
    file: Option<&Upload>,
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
            evidence_file: file.map(draft_file),
        };
        start_form(engine, &principal, &form, file).await?;
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
        evidence_files: file.map(new_upload).into_iter().collect(),
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
