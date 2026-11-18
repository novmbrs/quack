//! Multi-page `/case add` drafts (§11.6): stored in PostgreSQL with an
//! expiry and a page counter advanced by compare-and-set, so restarts and
//! double clicks are harmless. The final page applies the rule with the
//! operation key `draft:<id>` and the draft's version pinned.

use quack_core::DomainError;
use quack_core::case::{
    ContextInput, ContextProblem, MessageLink, fill_message_links, validate_context,
};
use quack_core::ids::{ChannelId, MemberId, RuleId};
use quack_core::permissions::Capability;
use quack_core::rule::{Field, RuleDefinition};
use quack_db::drafts::{self, Draft, NewDraft};
use quack_db::{DbError, Tx, rules};

use crate::cases::{Actor, Applied, ApplyError, ApplyRule, Receipt, apply_rule};
use crate::{Engine, OpKey, Principal, obs};

/// Fields per modal page (a Discord modal holds five inputs).
pub const FIELDS_PER_PAGE: usize = 5;

/// One submitted page of the case form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSubmit {
    /// The draft (its id is in the modal's custom id).
    pub draft: uuid::Uuid,
    /// The member the case is about.
    pub member: MemberId,
    /// The rule and the version the form was built from.
    pub rule: RuleId,
    /// The version.
    pub version: u32,
    /// The page (1-based).
    pub page: u8,
    /// The page's answers (field key → text).
    pub values: Vec<(String, String)>,
    /// Whether the receipt is private.
    pub private: bool,
    /// Where the command was used.
    pub channel: ChannelId,
}

/// What saving a page did.
#[derive(Clone, Debug, PartialEq)]
pub enum PageSaved {
    /// Saved; the form continues on this page (`case.draft.page_saved`).
    Next {
        /// The next page.
        page: u8,
        /// How many pages the form has.
        pages: u8,
    },
    /// The page has problems; its values are kept (`case.draft.page_invalid`).
    Invalid(Vec<ContextProblem>),
    /// A double click: the draft is already past this page.
    AlreadySaved {
        /// The draft's current page.
        page: u8,
    },
    /// The draft expired or belongs to someone else (`case.draft.expired`).
    Expired,
    /// The rule changed since the form was built; the draft is deleted.
    RuleChanged,
    /// Every page is saved and the rule was applied.
    Applied(Applied),
}

/// How many modal pages a rule version's form has.
pub fn pages(def: &RuleDefinition) -> u8 {
    let fields = def.context_fields().len();
    u8::try_from(fields.div_ceil(FIELDS_PER_PAGE).max(1)).unwrap_or(u8::MAX)
}

/// The fields of one page.
pub fn page_fields(def: &RuleDefinition, page: u8) -> Vec<Field> {
    def.context_fields()
        .iter()
        .skip(usize::from(page.saturating_sub(1)) * FIELDS_PER_PAGE)
        .take(FIELDS_PER_PAGE)
        .cloned()
        .collect()
}

/// Saves one page (§11.6 "Saving a page"); the last page applies the rule.
/// `receipt` is where the case's receipt goes when it is applied.
pub async fn save_page(
    engine: &Engine,
    principal: &Principal,
    submit: &PageSubmit,
    receipt: Option<Receipt>,
) -> Result<PageSaved, ApplyError> {
    let guild = principal.guild;
    obs::use_case("save_draft_page", Some(guild), async {
        let def = {
            let mut conn = engine.db().conn().await?;
            principal.require(&mut conn, Capability::Moderate).await??;
            rules::version(&mut conn, guild, submit.rule, submit.version)
                .await?
                .ok_or(DomainError::RuleNotFound)?
                .definition
        };
        // A resubmitted last page after the case committed (the case
        // transaction deleted the draft): answer with that case instead of
        // starting the draft again.
        if let Some(case) = applied_case(engine, principal, submit.draft).await? {
            return Ok(PageSaved::Applied(Applied::Replayed(case)));
        }
        let mut tx = engine.db().begin().await?;
        drafts::create(
            &mut tx,
            &NewDraft {
                id: submit.draft,
                guild,
                actor: principal.account,
                member: submit.member,
                rule: Some((submit.rule, submit.version)),
                private: submit.private,
                receipt_channel: submit.channel,
                source: None,
            },
        )
        .await?;
        let Some(draft) = drafts::lock(&mut tx, guild, principal.account, submit.draft).await?
        else {
            return Ok(PageSaved::Expired);
        };
        if draft.rule != Some((submit.rule, submit.version)) {
            drafts::delete(&mut tx, guild, principal.account, draft.id).await?;
            tx.commit().await?;
            return Ok(PageSaved::RuleChanged);
        }
        if draft.page > submit.page {
            tx.commit().await?;
            return Ok(PageSaved::AlreadySaved { page: draft.page });
        }
        let saved = save(&mut tx, &draft, &def, submit).await?;
        tx.commit().await?;
        match saved {
            Some(outcome) => Ok(outcome),
            None => Box::pin(apply(engine, principal, submit.draft, receipt)).await,
        }
    })
    .await
}

/// Stores the page; `None` when every page is now saved.
async fn save(
    tx: &mut Tx,
    draft: &Draft,
    def: &RuleDefinition,
    submit: &PageSubmit,
) -> Result<Option<PageSaved>, DbError> {
    let fields = page_fields(def, submit.page);
    let inputs: Vec<(String, ContextInput)> = submit
        .values
        .iter()
        .map(|(k, v)| (k.clone(), ContextInput::Text(v.clone())))
        .collect();
    let values = serde_json::Value::Object(
        submit
            .values
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect(),
    );
    if let Err(problems) = validate_context(&fields, draft.guild, &inputs) {
        drafts::save_page(
            tx,
            draft.guild,
            draft.actor,
            draft.id,
            submit.page,
            &values,
            false,
        )
        .await?;
        return Ok(Some(PageSaved::Invalid(problems)));
    }
    drafts::save_page(
        tx,
        draft.guild,
        draft.actor,
        draft.id,
        submit.page,
        &values,
        true,
    )
    .await?;
    let total = pages(def);
    if submit.page < total {
        return Ok(Some(PageSaved::Next {
            page: submit.page + 1,
            pages: total,
        }));
    }
    Ok(None)
}

/// The operation key of a draft's case.
fn draft_key(draft: uuid::Uuid) -> OpKey {
    OpKey::new("case_add", format!("draft:{draft}"))
}

/// The case a draft already became, found by its operation key.
async fn applied_case(
    engine: &Engine,
    principal: &Principal,
    draft: uuid::Uuid,
) -> Result<Option<quack_core::ids::CaseId>, DbError> {
    let scoped = draft_key(draft)
        .scoped(Some(principal.guild), Some(principal.account))
        .case_scoped();
    let mut conn = engine.db().conn().await?;
    Ok(
        quack_db::cases::find_by_operation_key(&mut conn, principal.guild, &scoped)
            .await?
            .map(|(case, _)| case),
    )
}

/// Applies a completed draft ("Try again" after a failed save, §11.6 step
/// 6): the operation key `draft:<id>` makes a repeat safe even if the
/// first try committed.
pub async fn apply(
    engine: &Engine,
    principal: &Principal,
    draft: uuid::Uuid,
    receipt: Option<Receipt>,
) -> Result<PageSaved, ApplyError> {
    let guild = principal.guild;
    let found = {
        let mut conn = engine.db().conn().await?;
        drafts::lock(&mut conn, guild, principal.account, draft).await?
    };
    let key = draft_key(draft);
    let Some(found) = found else {
        // The case transaction deletes its draft: a repeat after it
        // committed finds the case by its operation key (§4.3).
        return Ok(match applied_case(engine, principal, draft).await? {
            Some(case) => PageSaved::Applied(Applied::Replayed(case)),
            None => PageSaved::Expired,
        });
    };
    let Some((rule, version)) = found.rule else {
        return Ok(PageSaved::Expired);
    };
    let context: Vec<(String, ContextInput)> = found.values.as_object().map_or_default(|o| {
        o.iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ContextInput::Text(v.as_str().unwrap_or("").to_owned()),
                )
            })
            .collect()
    });
    let source = found.source.map(|(channel, message)| MessageLink {
        guild_id: guild,
        channel_id: channel,
        message_id: message,
    });
    let request = ApplyRule {
        guild,
        actor: Actor::Staff(principal.clone()),
        member: found.member,
        rule,
        pinned_version: Some(version),
        context,
        evidence_links: Vec::new(),
        source_message: source,
        receipt,
        replaces: None,
        op_key: key,
        draft: Some(draft),
        honeypot: None,
    };
    match Box::pin(apply_rule(engine, request)).await {
        Ok(applied) => Ok(PageSaved::Applied(applied)),
        Err(ApplyError::Refused(DomainError::RuleChanged)) => {
            let mut conn = engine.db().conn().await?;
            drafts::delete(&mut conn, guild, principal.account, draft).await?;
            Ok(PageSaved::RuleChanged)
        }
        Err(e) => Err(e),
    }
}

/// A draft started from the "Add case" message or member command, before a
/// rule is picked, or from `/case add` when its form opened (§11.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandDraft {
    /// A fresh draft id.
    pub draft: uuid::Uuid,
    /// The member.
    pub member: MemberId,
    /// The rule and the version the form was built from (`/case add`);
    /// `None` until the picker chooses one.
    pub rule: Option<(RuleId, u32)>,
    /// The message the command was used on.
    pub source: Option<MessageLink>,
    /// Private receipt (message command) or public (member command).
    pub private: bool,
    /// Where the command was used.
    pub channel: ChannelId,
}

/// Starts a draft: for the "Add case" commands (step 1), and for
/// `/case add` before its first form page opens, so the page's submit
/// finds who and where the case is for even after a restart.
pub async fn start(
    engine: &Engine,
    principal: &Principal,
    new: &CommandDraft,
) -> Result<(), ApplyError> {
    let mut conn = engine.db().conn().await?;
    principal.require(&mut conn, Capability::Moderate).await??;
    drafts::create(
        &mut conn,
        &NewDraft {
            id: new.draft,
            guild: principal.guild,
            actor: principal.account,
            member: new.member,
            rule: new.rule,
            private: new.private,
            receipt_channel: new.channel,
            source: new.source.map(|l| (l.channel_id, l.message_id)),
        },
    )
    .await?;
    Ok(())
}

/// The picker chose a rule (step 3): set the draft's rule at its current
/// version, fill empty message link fields from the source message, and
/// apply the rule unless required fields are left (then `Next { page: 1 }`).
pub async fn choose_rule(
    engine: &Engine,
    principal: &Principal,
    draft: uuid::Uuid,
    rule: RuleId,
    receipt: Option<Receipt>,
) -> Result<PageSaved, ApplyError> {
    let guild = principal.guild;
    let mut conn = engine.db().conn().await?;
    principal.require(&mut conn, Capability::Moderate).await??;
    let current = rules::get(&mut conn, guild, rule)
        .await?
        .ok_or(DomainError::RuleNotFound)?;
    if current.archived_at.is_some() {
        return Err(DomainError::RuleArchived.into());
    }
    let Some(found) = drafts::lock(&mut conn, guild, principal.account, draft).await? else {
        return Ok(PageSaved::Expired);
    };
    drafts::set_rule(
        &mut conn,
        guild,
        draft,
        principal.account,
        rule,
        current.current_version,
    )
    .await?;
    let fields = current.definition.context_fields();
    let mut filled = Vec::new();
    if let Some((channel, message)) = found.source {
        let link = MessageLink {
            guild_id: guild,
            channel_id: channel,
            message_id: message,
        };
        fill_message_links(&fields, &mut filled, &link);
    }
    if fields
        .iter()
        .any(|f| f.required && !filled.iter().any(|(k, _)| k == f.key.as_str()))
    {
        return Ok(PageSaved::Next {
            page: 1,
            pages: pages(&current.definition),
        });
    }
    let values = serde_json::Value::Object(
        filled
            .into_iter()
            .filter_map(|(k, v)| match v {
                ContextInput::Text(t) => Some((k, serde_json::Value::String(t))),
                _ => None,
            })
            .collect(),
    );
    drafts::save_page(
        &mut conn,
        guild,
        principal.account,
        draft,
        1,
        &values,
        false,
    )
    .await?;
    drop(conn);
    Box::pin(apply(engine, principal, draft, receipt)).await
}
