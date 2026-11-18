//! The "Add case" message and member commands and their rule picker
//! (§11.6 "From the Add case message or member command").

use quack_core::case::MessageLink;
use quack_core::ids::{CaseId, MemberId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{Person, SelectChoice, Source, TargetMessage};
use quack_engine::Principal;
use quack_engine::cache::CachedRule;
use quack_engine::drafts::{CommandDraft, choose_rule, start};
use quack_telemetry::labels::Reason;
use quack_text::actions::DraftPage;
use quack_text::case::{self, PickerRule, picker, picker_fill_form, picker_no_rules};
use quack_text::{Msg, Tone};

use super::outcome::{self, receipt};
use super::person;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth;
use crate::errors;

/// Rules per picker page (one select).
const PAGE: usize = 25;

/// "Add case" on a message: the private receipt, the message as evidence.
pub(crate) async fn from_message(p: Pending<'_>, message: &TargetMessage) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = begin_from_message(&a, message).await;
    a.finish_opt(result).await
}

async fn begin_from_message(
    a: &Acked<'_>,
    message: &TargetMessage,
) -> Result<Option<Msg>, Failure> {
    let author = message
        .author
        .as_ref()
        .ok_or_else(|| Failure::failed(quack_text::error::internal(), Reason::Invalid))?;
    let shown = quack_text::Person::new(
        author.id,
        author
            .global_name
            .clone()
            .unwrap_or_else(|| author.username.clone()),
    );
    let source = MessageLink {
        guild_id: auth::guild_of(a.ix)?,
        channel_id: message.channel_id,
        message_id: message.id,
    };
    begin(a, author.id.member(), &shown, Some(source), true).await
}

/// "Add case" on a member: a public receipt.
pub(crate) async fn for_member(p: Pending<'_>, member: &Person) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = begin(&a, member.account.id.member(), &person(member), None, false).await;
    a.finish_opt(result).await
}

/// Steps 1–2: authorize, insert the draft, and show the picker (or apply
/// the only active rule at once).
async fn begin(
    a: &Acked<'_>,
    member: MemberId,
    shown: &quack_text::Person,
    source: Option<MessageLink>,
    private: bool,
) -> Result<Option<Msg>, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let channel = a.ix.channel_id.ok_or_else(crate::router::expired)?;
    let draft = uuid::Uuid::now_v7();
    start(
        engine,
        &principal,
        &CommandDraft {
            draft,
            member,
            source,
            private,
            channel,
        },
    )
    .await
    .map_err(|e| outcome::apply_failed(&e, None, None))?;
    let rules = active_rules(a, &principal).await?;
    match rules.as_slice() {
        [] => Ok(Some(Msg::text(Tone::Neutral, picker_no_rules()))),
        [only] => {
            let only = only.clone();
            chosen(a, &principal, CaseId::from_uuid(draft), &only, shown).await
        }
        _ => Ok(Some(picker_page(
            shown,
            CaseId::from_uuid(draft),
            &rules,
            1,
        ))),
    }
}

/// The server's active rules, by name.
async fn active_rules(a: &Acked<'_>, principal: &Principal) -> Result<Vec<CachedRule>, Failure> {
    let engine = a.engine();
    let rules = engine
        .rules_cache()
        .get(engine.db(), principal.guild)
        .await
        .map_err(|e| errors::db(&e))?;
    let mut active = rules.active.clone();
    active.sort_by_key(|r| r.definition.name.to_lowercase());
    Ok(active)
}

/// One picker page (25 rules per page).
fn picker_page(shown: &quack_text::Person, draft: CaseId, rules: &[CachedRule], page: u32) -> Msg {
    let pages = u32::try_from(rules.len().div_ceil(PAGE).max(1)).unwrap_or(u32::MAX);
    let page = page.clamp(1, pages);
    let rows: Vec<PickerRule> = rules
        .iter()
        .skip(usize::try_from(page - 1).unwrap_or(0) * PAGE)
        .take(PAGE)
        .map(|r| PickerRule {
            id: r.id,
            name: r.definition.name.clone(),
            description: r.definition.description.clone(),
        })
        .collect();
    picker(shown, draft, &rows, page, pages)
}

/// The rule picker's pager (`rpp`).
pub(crate) async fn page(p: Pending<'_>, draft: CaseId, page: u32, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = page_now(&a, draft, page).await;
    a.finish(result).await
}

async fn page_now(a: &Acked<'_>, draft: CaseId, page: u32) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    auth::require(engine, &principal, Capability::Moderate).await?;
    let row = draft_row(a, &principal, draft).await?;
    let rules = active_rules(a, &principal).await?;
    let shown = quack_text::Person::unnamed(row.member.account());
    Ok(picker_page(&shown, draft, &rules, page))
}

async fn draft_row(
    a: &Acked<'_>,
    principal: &Principal,
    draft: CaseId,
) -> Result<quack_db::drafts::Draft, Failure> {
    let mut conn = a.engine().db().conn().await.map_err(|e| errors::db(&e))?;
    quack_db::drafts::lock(&mut conn, principal.guild, principal.account, draft.get())
        .await
        .map_err(|e| errors::db(&e))?
        .ok_or_else(|| Failure::refused(case::draft_expired(), Reason::Conflict))
}

/// A rule was picked (`rp`, step 3).
pub(crate) async fn picked(
    p: Pending<'_>,
    draft: CaseId,
    values: &[SelectChoice],
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = picked_now(&a, draft, values).await;
    a.finish_opt(result).await
}

async fn picked_now(
    a: &Acked<'_>,
    draft: CaseId,
    values: &[SelectChoice],
) -> Result<Option<Msg>, Failure> {
    let Some(SelectChoice::Rule(rule)) = values.first().copied() else {
        return Err(crate::router::expired());
    };
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    auth::require(engine, &principal, Capability::Moderate).await?;
    let row = draft_row(a, &principal, draft).await?;
    let rules = active_rules(a, &principal).await?;
    let Some(found) = rules.into_iter().find(|r| r.id == rule) else {
        return Err(errors::domain(
            quack_core::DomainError::RuleArchived,
            &quack_text::error::ErrorFacts::default(),
        ));
    };
    let shown = quack_text::Person::unnamed(row.member.account());
    chosen(a, &principal, draft, &found, &shown).await
}

/// Sets the draft's rule and applies it, or asks for the form (step 3).
/// The receipt replaces the picker (message command) or turns it into
/// `case.receipt.posted` (member command).
async fn chosen(
    a: &Acked<'_>,
    principal: &Principal,
    draft: CaseId,
    rule: &CachedRule,
    shown: &quack_text::Person,
) -> Result<Option<Msg>, Failure> {
    let engine = a.engine();
    let row = draft_row(a, principal, draft).await?;
    let receipt = receipt(a.ix, row.private, row.receipt_channel);
    let name = rule.definition.name.as_str();
    let saved = choose_rule(engine, principal, draft.get(), rule.id, Some(receipt))
        .await
        .map_err(|e| outcome::apply_failed(&e, Some(name), Some(draft)))?;
    let version = match &saved {
        quack_engine::drafts::PageSaved::Next { page: 1, .. } => {
            // The draft holds the exact version it was started with.
            draft_row(a, principal, draft)
                .await?
                .rule
                .map_or(rule.version, |(_, v)| v)
        }
        _ => rule.version,
    };
    let at = DraftPage {
        draft,
        rule: rule.id,
        version,
        page: 1,
    };
    if let quack_engine::drafts::PageSaved::Next { page: 1, .. } = saved {
        return Ok(Some(picker_fill_form(name, shown, at)));
    }
    outcome::page_saved(engine, principal, at, name, saved).await
}
