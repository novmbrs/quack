//! Edit context (`ce`/`cem`) on the case card. Evidence controls live in
//! `evidence` and `evidence_view`.

use quack_core::case::ContextInput;
use quack_core::ids::CaseId;
use quack_discord::interaction::{FieldValue, Source};
use quack_engine::cases::{CaseError, CaseView, edit_context, get};
use quack_engine::{CaseRef, Engine};
use quack_telemetry::labels::Reason;
use quack_text::actions::InputId;
use quack_text::case::{FormField, context_modal, context_saved, context_too_many_fields};
use quack_text::error::ErrorFacts;
use quack_text::model::Modal;
use quack_text::{Msg, Text, Tone};

use super::op_key;
use crate::ack::{Acked, Failure, Finish, Pending};
use crate::auth;
use crate::errors;

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
                // Context fields are never file uploads.
                FieldValue::YesNo(None) | FieldValue::Files(_) => String::new(),
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
