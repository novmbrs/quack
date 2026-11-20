//! Seeing and changing one evidence item from Discord (§7.4, §7.5,
//! §11.7): View evidence (`ev`) with its spoilered gallery, Messages around
//! it (`evs`), Show to member / Hide from member (`evv`), the note
//! (`evn`/`evnm`), and Remove (`evr`/`evrc`). Removed items are never
//! shown in Discord; every click is authorized again.

use quack_core::DomainError;
use quack_core::ids::{CaseId, ChannelId, EvidenceId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, Interaction, Source};
use quack_engine::cases::{self, CaseView, EvidenceItem};
use quack_engine::evidence::{gallery, remove_evidence, set_note, set_visibility};
use quack_engine::{CaseRef, Engine, Principal};
use quack_text::actions::InputId;
use quack_text::error::ErrorFacts;
use quack_text::evidence::{self as words, Surrounding, SurroundingLine};
use quack_text::model::Block;
use quack_text::{ChannelRef, Msg, Person, Text, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth;
use crate::errors;
use crate::router::text_of;

fn not_found() -> Failure {
    errors::domain(DomainError::EvidenceNotFound, &ErrorFacts::default())
}

/// The case as staff see it, after authorizing the actor.
async fn case_view(
    engine: &Engine,
    principal: &Principal,
    case: CaseId,
) -> Result<CaseView, Failure> {
    cases::get(engine, principal, CaseRef::Id(case))
        .await
        .map_err(|e| errors::case(&e, &ErrorFacts::default()))
}

/// The items Discord shows: every item that isn't removed (§7.5).
fn shown(v: &CaseView) -> Vec<&EvidenceItem> {
    v.evidence.iter().filter(|e| e.removed.is_none()).collect()
}

/// An item, its case, and its position among the items Discord shows
/// (`None` once removed).
struct Located {
    view: CaseView,
    item: EvidenceItem,
    position: Option<u32>,
}

/// Finds `evidence` in this server, after checking `moderate` so someone
/// who isn't staff learns nothing about which items exist.
async fn locate(
    engine: &Engine,
    principal: &Principal,
    evidence: EvidenceId,
) -> Result<Located, Failure> {
    auth::require(engine, principal, Capability::Moderate).await?;
    let case = {
        let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
        quack_db::evidence::get(&mut conn, principal.guild, evidence)
            .await
            .map_err(|e| errors::db(&e))?
            .ok_or_else(not_found)?
            .case
    };
    let view = case_view(engine, principal, case).await?;
    let item = view
        .evidence
        .iter()
        .find(|e| e.id == evidence)
        .cloned()
        .ok_or_else(not_found)?;
    let position = shown(&view)
        .iter()
        .position(|e| e.id == evidence)
        .and_then(|i| u32::try_from(i + 1).ok());
    Ok(Located {
        view,
        item,
        position,
    })
}

/// View evidence (`ev`): one item with a pager.
pub(crate) async fn view(p: Pending<'_>, case: CaseId, item: u32, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = view_now(&a, case, item).await;
    a.finish(result).await
}

async fn view_now(a: &Acked<'_>, case: CaseId, item: u32) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let v = case_view(engine, &principal, case).await?;
    Ok(item_msg(engine, &principal, &v, item).await)
}

/// Item `item` (1-based, clamped) of the items Discord shows, with fresh
/// gallery URLs; `evidence.view.none` when there are none.
async fn item_msg(engine: &Engine, principal: &Principal, v: &CaseView, item: u32) -> Msg {
    let items = shown(v);
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    if total == 0 {
        return Msg::text(Tone::Neutral, words::view_none(v.number));
    }
    let item = item.clamp(1, total);
    let e = items[usize::try_from(item - 1).unwrap_or(0)];
    // Best effort (§7.4): a gallery that can't be read now is left out.
    let media = gallery(engine, principal, CaseRef::Id(v.id), e.id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|f| (f.filename, f.url))
        .collect();
    words::view(&evidence_item(v, e, item, total, media))
}

/// The item re-rendered with a result line under its heading.
async fn item_with(engine: &Engine, principal: &Principal, located: &Located, line: Text) -> Msg {
    let Some(position) = located.position else {
        return Msg::text(Tone::Success, line);
    };
    let mut msg = item_msg(engine, principal, &located.view, position).await;
    let at = usize::from(matches!(msg.blocks.first(), Some(Block::Heading(_))));
    msg.blocks.insert(at, Block::Text(line));
    msg
}

fn evidence_item(
    v: &CaseView,
    e: &EvidenceItem,
    item: u32,
    total: u32,
    gallery: Vec<(String, String)>,
) -> words::EvidenceItem {
    words::EvidenceItem {
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
        gallery,
    }
}

const fn channel_ref(id: ChannelId) -> ChannelRef {
    ChannelRef { id, name: None }
}

/// Messages around it (`evs`): the messages saved around a saved message.
pub(crate) async fn surrounding(p: Pending<'_>, evidence: EvidenceId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = surrounding_now(&a, evidence).await;
    a.finish(result).await
}

async fn surrounding_now(a: &Acked<'_>, evidence: EvidenceId) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let found = locate(engine, &principal, evidence).await?;
    let Some(position) = found.position else {
        return Err(errors::domain(
            DomainError::EvidenceRemoved,
            &ErrorFacts::default(),
        ));
    };
    let lines = found
        .item
        .surrounding
        .iter()
        .flatten()
        .map(|m| SurroundingLine {
            author: Person::unnamed(m.author_id),
            sent_at: m.created_at,
            content: m.content.clone(),
            files: m.files,
            before: m.before,
        })
        .collect();
    Ok(words::surrounding(&Surrounding {
        case: found.view.id,
        number: found.view.number,
        item: position,
        author: found.item.author_id.map(Person::unnamed),
        lines,
    }))
}

/// Show to member / Hide from member (`evv`): sets visibility and shows
/// the item again.
pub(crate) async fn visibility(
    p: Pending<'_>,
    evidence: EvidenceId,
    visible: bool,
    source: Source,
) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = visibility_now(&a, evidence, visible).await;
    a.finish(result).await
}

async fn visibility_now(
    a: &Acked<'_>,
    evidence: EvidenceId,
    visible: bool,
) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let done = set_visibility(
        engine,
        &principal,
        None,
        evidence,
        visible,
        &op_key(a.ix, "evidence_visibility"),
    )
    .await
    .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
    let found = locate(engine, &principal, evidence).await?;
    let line = words::visibility_changed(done.number, visible);
    Ok(item_with(engine, &principal, &found, line).await)
}

/// Edit note / Add note (`evn`, §11.3's exception): the note modal
/// prefilled with the current note and its revision.
pub(crate) async fn open_note(p: Pending<'_>, evidence: EvidenceId) -> Finish {
    let ix = p.ix;
    p.prefilled_modal(|engine| async move { note_modal(engine, ix, evidence).await })
        .await
}

async fn note_modal(
    engine: &Engine,
    ix: &Interaction,
    evidence: EvidenceId,
) -> Result<quack_text::model::Modal, Failure> {
    let principal = auth::principal(engine, ix).await?;
    auth::require(engine, &principal, Capability::Moderate).await?;
    let mut conn = engine.db().conn().await.map_err(|e| errors::db(&e))?;
    let item = quack_db::evidence::get(&mut conn, principal.guild, evidence)
        .await
        .map_err(|e| errors::db(&e))?
        .ok_or_else(not_found)?;
    if item.is_removed() {
        return Err(errors::domain(
            DomainError::EvidenceRemoved,
            &ErrorFacts::default(),
        ));
    }
    let number = quack_db::cases::get(&mut conn, principal.guild, item.case)
        .await
        .map_err(|e| errors::db(&e))?
        .ok_or_else(not_found)?
        .number;
    Ok(words::note_modal(
        evidence,
        number,
        item.note_revision,
        item.note.as_deref(),
    ))
}

/// The note modal (`evnm`): saved only if the note is still at the
/// revision the modal showed (`error.note_changed`); blank clears it.
pub(crate) async fn save_note(
    p: Pending<'_>,
    evidence: EvidenceId,
    revision: u32,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let note = text_of(fields, &InputId::Note);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = save_note_now(&a, evidence, revision, &note).await;
    a.finish(result).await
}

async fn save_note_now(
    a: &Acked<'_>,
    evidence: EvidenceId,
    revision: u32,
    note: &str,
) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let done = set_note(
        engine,
        &principal,
        None,
        evidence,
        Some(note),
        revision,
        &op_key(a.ix, "evidence_note"),
    )
    .await
    .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
    let found = locate(engine, &principal, evidence).await?;
    let line = words::note_saved(done.number, note.trim().is_empty());
    Ok(item_with(engine, &principal, &found, line).await)
}

/// Remove (`evr`): asks to confirm (`evidence.remove.confirm`).
pub(crate) async fn ask_remove(p: Pending<'_>, evidence: EvidenceId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = ask_remove_now(&a, evidence).await;
    a.finish(result).await
}

async fn ask_remove_now(a: &Acked<'_>, evidence: EvidenceId) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let found = locate(engine, &principal, evidence).await?;
    let (case, number) = (found.view.id, found.view.number);
    Ok(match found.position {
        Some(item) => words::remove_confirm(case, number, evidence, item),
        None => removed_msg(case, number, false),
    })
}

/// "Yes, remove" (`evrc`): removes the item (§7.5).
pub(crate) async fn remove(p: Pending<'_>, evidence: EvidenceId, source: Source) -> Finish {
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = remove_now(&a, evidence).await;
    a.finish(result).await
}

async fn remove_now(a: &Acked<'_>, evidence: EvidenceId) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let done = remove_evidence(
        engine,
        &principal,
        None,
        evidence,
        &op_key(a.ix, "evidence_remove"),
    )
    .await
    .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
    Ok(removed_msg(
        done.case,
        done.number,
        done.changed || done.replayed,
    ))
}

/// `evidence.removed` with View evidence and Open case, to carry on.
fn removed_msg(case: CaseId, number: u64, changed: bool) -> Msg {
    let tone = if changed {
        Tone::Success
    } else {
        Tone::Neutral
    };
    Msg::text(tone, words::removed(number, changed)).buttons(vec![
        words::view_button(case),
        quack_text::word::open_case_button(case),
    ])
}
