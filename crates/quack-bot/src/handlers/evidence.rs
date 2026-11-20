//! Adding evidence from Discord (§7.1, §11.1): the Add evidence modal
//! (`ea`/`eam`) with its links, files, and note; `/case evidence`; and the
//! "Add as evidence" message command with its case select (`exs`) and case
//! number modal (`exn`/`exm`). Each answers `evidence.added` with Add
//! evidence and View evidence, so several things can be added in a row.

use quack_core::DomainError;
use quack_core::case::{ContextProblem, ContextProblemCode, MessageLink};
use quack_core::ids::{CaseId, ChannelId, MessageId};
use quack_core::permissions::Capability;
use quack_discord::interaction::{FieldValue, SelectChoice, Source, TargetMessage, Upload};
use quack_engine::CaseRef;
use quack_engine::cases::{AddEvidence, CaseError, add_evidence};
use quack_engine::drafts::DraftFile;
use quack_engine::evidence::{NewUpload, UploadSource, recent_cases};
use quack_telemetry::labels::Reason;
use quack_text::Msg;
use quack_text::actions::InputId;
use quack_text::error::ErrorFacts;
use quack_text::evidence::{Added, PickerCase, added_reply, picker, picker_modal};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth::{self, payload_capabilities};
use crate::errors;
use crate::router::text_of;

/// An interaction attachment as an upload the capture job downloads.
pub(crate) fn new_upload(u: &Upload) -> NewUpload {
    NewUpload {
        filename: u.filename.clone(),
        content_type: u.content_type.clone(),
        size_bytes: u.size_bytes,
        source: UploadSource::Discord {
            attachment_id: u.attachment_id,
            url: u.url.clone(),
        },
    }
}

/// An interaction attachment as a draft keeps it until the case exists
/// (§11.6).
pub(crate) fn draft_file(u: &Upload) -> DraftFile {
    DraftFile {
        attachment_id: u.attachment_id,
        filename: u.filename.clone(),
        content_type: u.content_type.clone(),
        size_bytes: u.size_bytes,
        url: u.url.clone(),
    }
}

/// Add evidence (`ea`): the modal (links, files, note). Its title names
/// the case number, which the custom id doesn't carry, so the number is
/// read under the prefilled-modal deadline (§11.3).
pub(crate) async fn open_modal(p: Pending<'_>, case: CaseId) -> Finish {
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
        let number = number
            .ok_or_else(|| errors::domain(DomainError::CaseNotFound, &ErrorFacts::default()))?;
        Ok(quack_text::evidence::modal(case, number))
    })
    .await
}

/// The files a modal's File Upload sent.
fn files_of(fields: &[(InputId, FieldValue)]) -> Vec<NewUpload> {
    fields
        .iter()
        .find_map(|(id, value)| match (id, value) {
            (InputId::Files, FieldValue::Files(files)) => {
                Some(files.iter().map(new_upload).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// A note as sent: blank is none.
fn note_of(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

/// The Add evidence modal (`eam`): links one per line, the uploaded files,
/// and a note; at least one link or file.
pub(crate) async fn submit_modal(
    p: Pending<'_>,
    case: CaseId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let request = AddEvidence {
        links: text_of(fields, &InputId::Links)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        files: files_of(fields),
        note: note_of(text_of(fields, &InputId::Note)),
    };
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = add(&a, CaseRef::Id(case), &request).await;
    a.finish(result).await
}

/// `/case evidence number [link] [file] [file2] [file3] [note]`.
pub(crate) async fn command(
    p: Pending<'_>,
    number: u64,
    links: &[String],
    files: &[Upload],
    note: Option<&str>,
) -> Finish {
    let request = AddEvidence {
        links: links.to_vec(),
        files: files.iter().map(new_upload).collect(),
        note: note.map(str::to_owned).and_then(note_of),
    };
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = add(&a, CaseRef::Number(number), &request).await;
    a.finish(result).await
}

/// Adds the request to the case and answers `evidence.added` with Add
/// evidence and View evidence.
async fn add(a: &Acked<'_>, case: CaseRef, request: &AddEvidence) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let facts = ErrorFacts {
        case_number: match case {
            CaseRef::Number(n) => Some(n),
            CaseRef::Id(_) => None,
        },
        ..ErrorFacts::default()
    };
    let added = add_evidence(
        engine,
        &principal,
        case,
        request,
        &op_key(a.ix, "case_add_evidence"),
    )
    .await
    .map_err(|e| match e {
        CaseError::LinksInvalid(_) => Failure::refused(
            quack_text::case::context_problem(
                &ContextProblem {
                    key: String::new(),
                    code: ContextProblemCode::NotAMessageLink,
                },
                Some("Message links"),
                None,
            ),
            Reason::Invalid,
        ),
        other => errors::case(&other, &facts),
    })?;
    Ok(added_reply(
        added.case,
        &Added {
            number: added.number,
            new: added.new,
            retrying: added.retrying,
            restored: added.restored,
            already_saved: added.already_saved,
            files: added.files,
        },
    ))
}

/// The link of a message the "Add as evidence" command was used on.
fn link_of(a: &Acked<'_>, channel: ChannelId, message: MessageId) -> Result<String, Failure> {
    Ok(MessageLink {
        guild_id: auth::guild_of(a.ix)?,
        channel_id: channel,
        message_id: message,
    }
    .to_url())
}

/// "Add as evidence" (§11.1): the picker of the author's and the actor's
/// recent cases, and Enter a case number.
pub(crate) async fn from_message(p: Pending<'_>, message: &TargetMessage) -> Finish {
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = show_picker(&a, message).await;
    a.finish(result).await
}

async fn show_picker(a: &Acked<'_>, message: &TargetMessage) -> Result<Msg, Failure> {
    let engine = a.engine();
    let principal = auth::principal(engine, a.ix).await?;
    let author = message.author.as_ref().map(|u| u.id.member());
    let cases = recent_cases(engine, &principal, author)
        .await
        .map_err(|e| errors::case(&e, &ErrorFacts::default()))?;
    let cases: Vec<PickerCase> = cases
        .into_iter()
        .map(|c| PickerCase {
            case: c.id,
            number: c.number,
            rule: c.rule_name,
            created_at: c.created_at,
            voided: c.voided,
        })
        .collect();
    Ok(picker(message.channel_id, message.id, &cases))
}

/// The picker's case select (`exs`): adds the message to the chosen case.
pub(crate) async fn picked(
    p: Pending<'_>,
    channel: ChannelId,
    message: MessageId,
    values: &[SelectChoice],
    source: Source,
) -> Finish {
    let Some(case) = values.iter().find_map(|v| match v {
        SelectChoice::Case(c) => Some(*c),
        _ => None,
    }) else {
        return p.fail(crate::router::expired()).await;
    };
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = match link_of(&a, channel, message) {
        Ok(link) => add(&a, CaseRef::Id(case), &AddEvidence::links(&[link])).await,
        Err(failure) => Err(failure),
    };
    a.finish(result).await
}

/// Enter a case number (`exn`): the modal (no stored values). Someone the
/// payload shows without `moderate` is refused instead.
pub(crate) async fn open_number(p: Pending<'_>, channel: ChannelId, message: MessageId) -> Finish {
    if !payload_capabilities(p.ix, p.config().as_deref()).moderate {
        return p.fail(auth::missing(Capability::Moderate)).await;
    }
    p.modal(&picker_modal(channel, message)).await
}

/// The case number modal (`exm`): adds the message to that case, with the
/// note (`error.case_number_invalid` for a number that isn't one).
pub(crate) async fn submit_number(
    p: Pending<'_>,
    channel: ChannelId,
    message: MessageId,
    fields: &[(InputId, FieldValue)],
    source: Source,
) -> Finish {
    let number = text_of(fields, &InputId::CaseNumber)
        .trim()
        .trim_start_matches('#')
        .parse::<u64>()
        .ok()
        .filter(|n| *n >= 1);
    let note = note_of(text_of(fields, &InputId::Note));
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = match (number, link_of(&a, channel, message)) {
        (_, Err(failure)) => Err(failure),
        (None, Ok(_)) => invalid_number(&a).await,
        (Some(number), Ok(link)) => {
            let request = AddEvidence {
                links: vec![link],
                files: Vec::new(),
                note,
            };
            add(&a, CaseRef::Number(number), &request).await
        }
    };
    a.finish(result).await
}

/// `error.case_number_invalid`, once the clicker is known to be staff (so
/// an outsider learns nothing but that they can't).
async fn invalid_number(a: &Acked<'_>) -> Result<Msg, Failure> {
    let principal = auth::principal(a.engine(), a.ix).await?;
    auth::require(a.engine(), &principal, Capability::Moderate).await?;
    Err(Failure::refused(
        quack_text::error::case_number_invalid(),
        Reason::Invalid,
    ))
}
