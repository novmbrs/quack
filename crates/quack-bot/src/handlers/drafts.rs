//! The pages of a multi-page `/case add` form (§11.6): Continue (`cn`),
//! Try again on a page with problems (`cr`), the page modal (`cd`), and
//! Try again after a failed save (`cs`).

use quack_core::case::ContextValue;
use quack_core::ids::{CaseId, ChannelId, MemberId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::Engine;
use quack_engine::cache::CachedRule;
use quack_engine::drafts::{PageSaved, PageSubmit, apply, pages, save_page};
use quack_telemetry::labels::Reason;
use quack_text::actions::{DraftPage, InputId};
use quack_text::case::{self, form_modal};
use quack_text::model::Modal;
use quack_text::{Msg, Tone};

use super::case_add::form_fields;
use super::outcome::{self, receipt};
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;

/// The cached rule at the page's version, if the cache has it.
fn cached_rule(p: &Pending<'_>, at: DraftPage) -> Option<CachedRule> {
    p.rules()
        .and_then(|r| r.active(at.rule).cloned())
        .filter(|r| r.version == at.version)
}

/// The rule's current name from the cache (any version), for messages.
fn rule_name(p: &Pending<'_>, at: DraftPage) -> String {
    p.rules()
        .and_then(|r| r.active(at.rule).map(|c| c.definition.name.clone()))
        .unwrap_or_default()
}

/// Continue / Fill in the form (`cn`): the page's modal from the rules
/// cache, with no stored values. A rule that changed answers
/// `case.draft.rule_changed`.
pub(crate) async fn open_page(p: Pending<'_>, at: DraftPage) -> Finish {
    if !payload_capabilities(p.ix, p.config().as_deref()).moderate {
        return p.fail(auth::missing(Capability::Moderate)).await;
    }
    let Some(rule) = cached_rule(&p, at) else {
        let name = rule_name(&p, at);
        let msg = Msg::text(Tone::Warning, case::draft_rule_changed(&name));
        return p.reply(&msg, Finish::Refused(Reason::Conflict)).await;
    };
    let modal = form_modal(
        &rule.definition.name,
        at,
        pages(&rule.definition),
        &form_fields(&rule, at.page),
    );
    p.modal(&modal).await
}

/// Try again on a page with problems (`cr`, §11.3's exception): authorize,
/// read the draft's saved answers, and re-open the page prefilled.
pub(crate) async fn retry_page(p: Pending<'_>, at: DraftPage) -> Finish {
    let ix = p.ix;
    let rule = cached_rule(&p, at);
    let name = rule_name(&p, at);
    p.prefilled_modal(|engine| async move {
        let principal = auth::principal(engine, ix).await?;
        auth::require(engine, &principal, Capability::Moderate).await?;
        let Some(rule) = rule else {
            return Err(Failure::refused(
                case::draft_rule_changed(&name),
                Reason::Conflict,
            ));
        };
        let draft = read_draft(engine, principal.guild, principal.account, at.draft)
            .await?
            .ok_or_else(|| Failure::refused(case::draft_expired(), Reason::Conflict))?;
        Ok(prefilled(&rule, at, &draft.values))
    })
    .await
}

/// The page's modal with the answers the moderator sent.
fn prefilled(rule: &CachedRule, at: DraftPage, values: &serde_json::Value) -> Modal {
    let mut fields = form_fields(rule, at.page);
    for f in &mut fields {
        let sent = values
            .get(f.key.as_str())
            .and_then(serde_json::Value::as_str);
        f.value = match (f.kind, sent) {
            (quack_core::rule::FieldKind::Boolean, Some("yes")) => {
                Some(ContextValue::Boolean(true))
            }
            (quack_core::rule::FieldKind::Boolean, Some("no")) => {
                Some(ContextValue::Boolean(false))
            }
            (quack_core::rule::FieldKind::Boolean, _) | (_, None) => None,
            (_, Some(text)) => Some(ContextValue::Text(text.to_owned())),
        };
    }
    form_modal(&rule.definition.name, at, pages(&rule.definition), &fields)
}

/// The draft row, when it exists for this moderator.
async fn read_draft(
    engine: &Engine,
    guild: quack_core::ids::GuildId,
    actor: quack_core::ids::AccountId,
    draft: CaseId,
) -> Result<Option<quack_db::drafts::Draft>, Failure> {
    let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
    quack_db::drafts::lock(&mut conn, guild, actor, draft.get())
        .await
        .map_err(|e| errors::db(&e))
}

/// Who and where a draft is for, from its row (`/case add` writes it when
/// the form opens; the "Add case" commands when they start).
async fn draft_target(
    a: &Acked<'_>,
    guild: quack_core::ids::GuildId,
    actor: quack_core::ids::AccountId,
    draft: CaseId,
) -> Result<Option<(MemberId, bool, ChannelId)>, Failure> {
    Ok(read_draft(a.engine(), guild, actor, draft)
        .await?
        .map(|d| (d.member, d.private, d.receipt_channel)))
}

/// The answers a page's modal sent: field key → text (`yes`/`no` for
/// Yes / No selects).
fn values(fields: &[(InputId, FieldValue)]) -> Vec<(String, String)> {
    fields
        .iter()
        .filter_map(|(id, value)| {
            let InputId::Field(key) = id else {
                return None;
            };
            let text = match value {
                FieldValue::Text(t) => t.clone(),
                FieldValue::YesNo(Some(true)) => "yes".to_owned(),
                FieldValue::YesNo(Some(false)) => "no".to_owned(),
                FieldValue::YesNo(None) => String::new(),
            };
            Some((key.as_str().to_owned(), text))
        })
        .collect()
}

/// A page's modal (`cd`): save it by compare-and-set; the last page
/// applies the rule with the operation key `draft:<id>` (§11.6).
pub(crate) async fn submit(
    p: Pending<'_>,
    at: DraftPage,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let rule_name = rule_name(&p, at);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = save(&a, at, fields, &rule_name).await;
    a.finish_opt(result).await
}

async fn save(
    a: &Acked<'_>,
    at: DraftPage,
    fields: &[(InputId, FieldValue)],
    rule: &str,
) -> Result<Option<Msg>, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let Some((member, private, channel)) =
        draft_target(a, principal.guild, principal.account, at.draft).await?
    else {
        // No form and no row: the draft expired, or it was already used.
        return retry(a, &principal, at, rule, true).await;
    };
    let submit = PageSubmit {
        draft: at.draft.get(),
        member,
        rule: at.rule,
        version: at.version,
        page: at.page,
        values: values(fields),
        private,
        channel,
    };
    let receipt = receipt(a.ix, private, channel);
    let saved = save_page(engine, &principal, &submit, Some(receipt)).await;
    match saved {
        Ok(PageSaved::Expired) => retry(a, &principal, at, rule, private).await,
        Ok(PageSaved::AlreadySaved { page }) if page > total(a, principal.guild, at) => {
            retry(a, &principal, at, rule, private).await
        }
        Ok(saved) => outcome::page_saved(engine, &principal, at, rule, saved).await,
        Err(e) => Err(outcome::apply_failed(&e, Some(rule), Some(at.draft))),
    }
}

/// How many pages the form has (from the cache; 1 when unknown).
fn total(a: &Acked<'_>, guild: quack_core::ids::GuildId, at: DraftPage) -> u8 {
    a.engine()
        .rules_cache()
        .cached(guild)
        .and_then(|r| r.active(at.rule).map(|c| pages(&c.definition)))
        .unwrap_or(1)
}

/// Applies the completed draft again; the operation key replays a case the
/// first try saved, and an expired draft says so.
async fn retry(
    a: &Acked<'_>,
    principal: &quack_engine::Principal,
    at: DraftPage,
    rule: &str,
    private: bool,
) -> Result<Option<Msg>, Failure> {
    let channel = a.ix.channel_id.ok_or_else(crate::router::expired)?;
    let receipt = receipt(a.ix, private, channel);
    match apply(a.engine(), principal, at.draft.get(), Some(receipt)).await {
        Ok(saved) => outcome::page_saved(a.engine(), principal, at, rule, saved).await,
        Err(e) => Err(outcome::apply_failed(&e, Some(rule), Some(at.draft))),
    }
}

/// Try again after a failed save (`cs`): apply the completed draft again.
pub(crate) async fn save_again(p: Pending<'_>, draft: CaseId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = save_again_now(&a, draft).await;
    a.finish_opt(result).await
}

async fn save_again_now(a: &Acked<'_>, draft: CaseId) -> Result<Option<Msg>, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let row = read_draft(engine, principal.guild, principal.account, draft).await?;
    // Without the row, a case it saved is found by its operation key; the
    // receipt then stays private (never posted by guess).
    let private = row.as_ref().is_none_or(|d| d.private);
    let rule = row.as_ref().and_then(|d| d.rule);
    let at = DraftPage {
        draft,
        rule: rule.map_or(quack_core::ids::RuleId::from_uuid(uuid::Uuid::nil()), |r| {
            r.0
        }),
        version: rule.map_or(0, |r| r.1),
        page: 1,
    };
    let name = rule
        .and_then(|(id, _)| {
            engine
                .rules_cache()
                .cached(principal.guild)
                .and_then(|r| r.active(id).map(|c| c.definition.name.clone()))
        })
        .unwrap_or_default();
    retry(a, &principal, at, &name, private).await
}
