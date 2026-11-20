//! Case card controls that change or show a case's details: Edit context
//! (`ce`/`cem`), Add evidence (`ea`/`eam`), and View evidence (`ev`).

use quack_core::case::ContextInput;
use quack_core::ids::{CaseId, ChannelId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::cases::{self, CaseError, CaseView, EvidenceItem, edit_context, get};
use quack_engine::{CaseRef, Engine};
use quack_telemetry::labels::Reason;
use quack_text::actions::InputId;
use quack_text::case::{FormField, context_modal, context_saved, context_too_many_fields};
use quack_text::error::ErrorFacts;
use quack_text::model::Modal;
use quack_text::{ChannelRef, Msg, Person, Text, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth;
use crate::errors;
use crate::router::text_of;

/// Fields an edit-context modal holds (one Discord modal).
const MODAL_FIELDS: usize = 5;

/// The case as staff see it, after authorizing the actor.
async fn view(
    engine: &Engine,
    ix: &quack_discord::interaction::Interaction,
    case: CaseId,
) -> Result<CaseView, Failure> {
    let principal = auth::principal(engine, ix).await?;
    get(engine, &principal, CaseRef::Id(case))
        .await
        .map_err(|e| errors::case(&e, &ErrorFacts::default()))
}

/// Edit context (`ce`, §11.3's exception): the modal prefilled with the
/// current context; more than 5 fields go to the dashboard.
pub(crate) async fn open_context(p: Pending<'_>, case: CaseId) -> Finish {
    let ix = p.ix;
    p.prefilled_modal(|engine| async move {
        let v = view(engine, ix, case).await?;
        let guild = auth::guild_of(ix)?;
        if v.fields.len() > MODAL_FIELDS {
            let url = engine.links().case(guild, v.number);
            let msg = context_too_many_fields(v.number, &url);
            return Err(Failure::refused(Text::new(), Reason::Invalid).shown_as(msg));
        }
        Ok(context_form(&v))
    })
    .await
}

fn context_form(v: &CaseView) -> Modal {
    let values = v.context.clone().with_kinds(&v.fields);
    let fields: Vec<FormField> = v
        .fields
        .iter()
        .map(|f| FormField {
            key: f.key.clone(),
            label: f.label.clone(),
            kind: f.kind,
            required: f.required,
            value: values.get(&f.key).cloned(),
        })
        .collect();
    context_modal(v.id, v.number, v.context_revision, &fields)
}

/// The edit-context modal (`cem`): saved only if the context is still at
/// the revision the modal showed.
pub(crate) async fn save_context(
    p: Pending<'_>,
    case: CaseId,
    revision: u32,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = save(&a, case, revision, fields).await;
    a.finish(result).await
}

async fn save(
    a: &Acked<'_>,
    case: CaseId,
    revision: u32,
    fields: &[(InputId, FieldValue)],
) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let context: Vec<(String, ContextInput)> = fields
        .iter()
        .filter_map(|(id, value)| {
            let InputId::Field(key) = id else { return None };
            let text = match value {
                FieldValue::Text(t) => t.clone(),
                FieldValue::YesNo(Some(true)) => "yes".to_owned(),
                FieldValue::YesNo(Some(false)) => "no".to_owned(),
                FieldValue::YesNo(None) => String::new(),
            };
            Some((key.as_str().to_owned(), ContextInput::Text(text)))
        })
        .collect();
    match edit_context(
        engine,
        &principal,
        CaseRef::Id(case),
        &context,
        revision,
        &op_key(a.ix, "case_edit_context"),
    )
    .await
    {
        Ok(done) => Ok(Msg::text(
            Tone::Success,
            context_saved(done.number, done.new_evidence),
        )),
        Err(CaseError::ContextInvalid(problems)) => {
            let v = view(engine, a.ix, case).await?;
            let lines = errors::context_problems(&problems, &v.fields);
            let mut text = Text::new();
            for (i, line) in lines.into_iter().enumerate() {
                text = if i == 0 {
                    line
                } else {
                    text.s("\n").then(line)
                };
            }
            Err(Failure::refused(text, Reason::Invalid))
        }
        Err(e) => Err(errors::case(&e, &ErrorFacts::default())),
    }
}

/// Add evidence (`ea`): the links modal. Its title names the case number,
/// which the custom id doesn't carry, so the number is read under the
/// prefilled-modal deadline (§11.3).
pub(crate) async fn open_evidence(p: Pending<'_>, case: CaseId) -> Finish {
    let ix = p.ix;
    p.prefilled_modal(|engine| async move {
        let principal = auth::principal(engine, ix).await?;
        auth::require(engine, &principal, Capability::Moderate).await?;
        let number = {
            let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
            quack_db::cases::get(&mut conn, principal.guild, case)
                .await
                .map_err(|e| errors::db(&e))?
                .map(|c| c.number)
        };
        let number = number.ok_or_else(|| {
            errors::domain(
                quack_core::DomainError::CaseNotFound,
                &ErrorFacts::default(),
            )
        })?;
        Ok(quack_text::evidence::modal(case, number))
    })
    .await
}

/// The add-evidence modal (`eam`): one message link per line.
pub(crate) async fn add_evidence(
    p: Pending<'_>,
    case: CaseId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let links: Vec<String> = text_of(fields, &InputId::Links)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = add(&a, case, &links).await;
    a.finish(result).await
}

async fn add(a: &Acked<'_>, case: CaseId, links: &[String]) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let added = cases::add_evidence(
        engine,
        &principal,
        CaseRef::Id(case),
        &cases::AddEvidence::links(links),
        &op_key(a.ix, "case_add_evidence"),
    )
    .await
    .map_err(|e| match e {
        CaseError::LinksInvalid(_) => Failure::refused(
            quack_text::case::context_problem(
                &quack_core::case::ContextProblem {
                    key: String::new(),
                    code: quack_core::case::ContextProblemCode::NotAMessageLink,
                },
                Some("Message links"),
                None,
            ),
            Reason::Invalid,
        ),
        other => errors::case(&other, &ErrorFacts::default()),
    })?;
    Ok(Msg::text(
        Tone::Success,
        quack_text::evidence::added(&quack_text::evidence::Added {
            number: added.number,
            new: added.new,
            retrying: added.retrying,
            restored: added.restored,
            already_saved: added.already_saved,
            files: added.files,
        }),
    ))
}

/// View evidence (`ev`): one item with a pager.
pub(crate) async fn view_evidence(
    p: Pending<'_>,
    case: CaseId,
    item: u32,
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show_item(&a, case, item).await;
    a.finish(result).await
}

async fn show_item(a: &Acked<'_>, case: CaseId, item: u32) -> Result<Msg, Failure> {
    let mut v = view(a.engine(), a.ix, case).await?;
    // Removed items are only listed on the dashboard (§7.5).
    v.evidence.retain(|e| e.removed.is_none());
    let total = u32::try_from(v.evidence.len()).unwrap_or(u32::MAX);
    if total == 0 {
        return Ok(Msg::text(
            Tone::Neutral,
            quack_text::evidence::view_none(v.number),
        ));
    }
    let item = item.clamp(1, total);
    let e = &v.evidence[usize::try_from(item - 1).unwrap_or(0)];
    Ok(quack_text::evidence::view(&evidence_item(
        &v, e, item, total,
    )))
}

fn evidence_item(
    v: &CaseView,
    e: &EvidenceItem,
    item: u32,
    total: u32,
) -> quack_text::evidence::EvidenceItem {
    quack_text::evidence::EvidenceItem {
        case: v.id,
        number: v.number,
        evidence: e.id,
        kind: e.kind,
        item,
        total,
        state: e.state,
        failure: e.failure,
        added_by: e.added_by.map(Person::unnamed),
        added_at: e.created_at,
        author: e.author_id.map(Person::unnamed),
        channel: e.channel_id.map(channel_ref),
        sent_at: e.message_created_at,
        edited_at: e.message_edited_at,
        url: e.url.clone(),
        content: e.content.clone(),
        embeds: e
            .embeds
            .iter()
            .map(|em| em.title.clone().unwrap_or_else(|| em.kind.clone()))
            .collect(),
        files: e
            .files
            .iter()
            .map(|f| (f.filename.clone(), f.copy_state))
            .collect(),
        files_not_saved: e.files_not_saved,
        channel_not_staff_only: false,
        note: e.note.clone(),
        visible_to_member: e.visible_to_member,
        surrounding: e
            .surrounding
            .as_ref()
            .map(|s| u32::try_from(s.len()).unwrap_or(u32::MAX)),
        gallery: Vec::new(),
    }
}

const fn channel_ref(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}
