//! `/ops` and the ops buttons (§21.8). Only operators, only in the ops
//! server; anyone else gets `ops.not_allowed`, with nothing read first.

use quack_core::ids::JobId;
use quack_db::DbError;
use quack_discord::interaction::{OpsRequest, Source};
use quack_engine::port::{FileEdit, Outcome};
use quack_ops::{Answer, Ops};
use quack_telemetry::labels::Reason;
use quack_text::Msg;

use crate::ack::{Failure, Finish, Pending};
use crate::errors;

/// The ops handle when the clicker is an operator in the ops server.
fn operator(p: &Pending<'_>) -> Option<Ops> {
    let ops = p.shared().ops.get()?;
    let account = p.ix.invoker.as_ref()?.account.id;
    ops.is_operator(p.ix.guild_id, account).then(|| ops.clone())
}

fn not_allowed() -> Failure {
    Failure::refused(quack_text::ops::not_allowed(), Reason::NotAllowed)
}

fn answered(result: Result<Answer, DbError>) -> Result<Msg, Failure> {
    match result {
        Ok(Ok(msg)) => Ok(msg),
        Ok(Err(text)) => Err(Failure::refused(text, Reason::Invalid)),
        Err(e) => Err(errors::db(&e)),
    }
}

fn read(result: Result<Msg, DbError>) -> Result<Msg, Failure> {
    result.map_err(|e| errors::db(&e))
}

/// `/ops status`, `/ops server`, `/ops jobs`.
pub(crate) async fn command(p: Pending<'_>, request: &OpsRequest) -> Finish {
    let Some(ops) = operator(&p) else {
        return p.fail(not_allowed()).await;
    };
    let a = match p.defer().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = match request {
        OpsRequest::Status => read(ops.status().await),
        OpsRequest::Server(id) => answered(ops.server(id).await),
        OpsRequest::Jobs => read(ops.dead_jobs(1).await),
    };
    a.finish(result).await
}

/// **Refresh** on the board or a private copy: re-renders it in place.
pub(crate) async fn refresh(p: Pending<'_>) -> Finish {
    let Some(ops) = operator(&p) else {
        return p.fail(not_allowed()).await;
    };
    let a = match p.defer_update().await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = read(ops.status().await);
    a.finish(result).await
}

/// A `/ops jobs` page.
pub(crate) async fn page(p: Pending<'_>, page: u32, source: Source) -> Finish {
    let Some(ops) = operator(&p) else {
        return p.fail(not_allowed()).await;
    };
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let result = read(ops.dead_jobs(page).await);
    a.finish(result).await
}

/// What a job button does.
#[derive(Clone, Copy)]
pub(crate) enum JobControl {
    /// **Requeue**.
    Requeue,
    /// **Dismiss**.
    Dismiss,
}

/// **Requeue** or **Dismiss** a dead job. On a private `/ops jobs` list
/// the list becomes the result; on an alert in `#ops-alerts` the alert is
/// edited to say who did it, and the clicker gets a private copy.
pub(crate) async fn job(p: Pending<'_>, job: JobId, control: JobControl, source: Source) -> Finish {
    let Some(ops) = operator(&p) else {
        return p.fail(not_allowed()).await;
    };
    let by = p.ix.invoker.as_ref().map(|i| i.account.id);
    let a = match p.defer_from(source).await {
        Ok(a) => a,
        Err(finish) => return finish,
    };
    let Some(by) = by else {
        return a.finish(Err(not_allowed())).await;
    };
    let result = answered(match control {
        JobControl::Requeue => ops.requeue(job, by).await,
        JobControl::Dismiss => ops.dismiss(job, by).await,
    });
    if !source.ephemeral
        && let (Ok(msg), Some(channel), Some(message)) =
            (&result, a.ix.channel_id, source.message_id)
    {
        let edited = a
            .engine()
            .discord()
            .edit(channel, message, msg, &FileEdit::None)
            .await;
        if !matches!(edited, Outcome::Done(_)) {
            tracing::warn!(outcome = %edited.code(), "couldn't edit an ops alert");
        }
    }
    a.finish(result).await
}
