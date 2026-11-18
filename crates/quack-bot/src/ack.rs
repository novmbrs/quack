//! The one acknowledgement helper (§11.3).
//!
//! A handler starts with a [`Pending`] interaction, which offers only
//! in-memory reads (the rules and config caches, dashboard links). The
//! engine, and with it every database and Discord call, is reachable only
//! through the [`Acked`] handle an acknowledgement returns, so slow work
//! cannot run before the ack. The single exception, a modal prefilled with
//! stored values, goes through [`Pending::prefilled_modal`], which bounds
//! its read to 300 ms (§11.3).

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quack_core::ids::{CaseId, GuildId};
use quack_discord::Outcome;
use quack_discord::interaction::{Interaction, Source};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::cache::{Choice, GuildRules};
use quack_engine::{Engine, GuildConfig};
use quack_telemetry::labels::Reason;
use quack_telemetry::metrics;
use quack_telemetry::spans::{self, SpanEvent};
use quack_text::model::Modal;
use quack_text::{Msg, Text, Tone};

use crate::bot::Shared;

/// The ack must go out within this long of the interaction's snowflake
/// time (§11.3).
const ACK_DEADLINE: time::Duration = time::Duration::milliseconds(2_500);

/// How long the exception read before a prefilled modal may take (§11.3).
pub(crate) const PREFILL_DEADLINE: Duration = Duration::from_millis(300);

/// How an interaction ended, for its span and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Finish {
    /// Quack did what was asked.
    Done,
    /// Quack said no for a reason the person can act on.
    Refused(Reason),
    /// Something went wrong on Quack's or Discord's side.
    Failed(Reason),
}

/// A refusal or failure with its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    /// What the person reads.
    pub(crate) text: Text,
    /// How the interaction ended.
    pub(crate) finish: Finish,
    /// A whole message to show instead of the text alone (a **Try again**
    /// button, a dashboard link).
    pub(crate) custom: Option<Box<Msg>>,
}

impl Failure {
    /// A refusal the person can act on.
    pub(crate) const fn refused(text: Text, reason: Reason) -> Self {
        Self {
            text,
            finish: Finish::Refused(reason),
            custom: None,
        }
    }

    /// A failure on Quack's or Discord's side.
    pub(crate) const fn failed(text: Text, reason: Reason) -> Self {
        Self {
            text,
            finish: Finish::Failed(reason),
            custom: None,
        }
    }

    /// The same failure, offering **Try again** for a completed draft
    /// (§11.6 step 6).
    #[must_use]
    pub(crate) fn with_retry(mut self, draft: CaseId) -> Self {
        self.custom = Some(Box::new(quack_text::case::draft_save_failed(
            self.text.clone(),
            draft,
        )));
        self
    }

    /// The same failure, shown as `msg`.
    #[must_use]
    pub(crate) fn shown_as(mut self, msg: Msg) -> Self {
        self.custom = Some(Box::new(msg));
        self
    }

    /// The message shown for it.
    pub(crate) fn msg(&self) -> Msg {
        match &self.custom {
            Some(msg) => (**msg).clone(),
            None => Msg::text(Tone::Danger, self.text.clone()),
        }
    }
}

/// An interaction that has not been acknowledged yet.
pub(crate) struct Pending<'a> {
    /// The interaction.
    pub(crate) ix: &'a Interaction,
    shared: &'a Shared,
    received: Instant,
}

impl<'a> Pending<'a> {
    /// Wraps an interaction received at `received`.
    pub(crate) const fn new(ix: &'a Interaction, shared: &'a Shared, received: Instant) -> Self {
        Self {
            ix,
            shared,
            received,
        }
    }

    /// The server, when the interaction happened in one.
    pub(crate) const fn guild(&self) -> Option<GuildId> {
        self.ix.guild_id
    }

    /// The server's rules from memory (never the database).
    pub(crate) fn rules(&self) -> Option<Arc<GuildRules>> {
        self.guild()
            .and_then(|g| self.shared.engine.rules_cache().cached(g))
    }

    /// The server's settings from memory (never the database).
    pub(crate) fn config(&self) -> Option<Arc<GuildConfig>> {
        self.guild()
            .and_then(|g| self.shared.engine.config_cache().cached(g))
    }

    /// The dashboard's public origin.
    pub(crate) fn public_url(&self) -> &'a str {
        &self.shared.engine.config().public_url
    }

    /// Autocomplete choices from the rules cache (§11.1).
    pub(crate) fn rule_choices(&self, query: &str, include_archived: bool) -> Vec<Choice> {
        self.guild().map_or_default(|g| {
            self.shared
                .engine
                .rules_cache()
                .autocomplete(g, query, include_archived)
        })
    }

    /// The bot's shared state (forms, modules), for in-memory reads.
    pub(crate) const fn shared(&self) -> &'a Shared {
        self.shared
    }

    /// Records the ack on the metric and the span.
    fn acked(&self, outcome: &Outcome<()>, how: &'static str) -> Result<(), Finish> {
        let latency = self.received.elapsed();
        if let Some(created) = self.ix.created_at()
            && time::OffsetDateTime::now_utc() - created > ACK_DEADLINE
        {
            tracing::warn!(
                interaction.id = self.ix.id,
                "an interaction was acknowledged more than 2.5 s after Discord created it"
            );
        }
        metrics()
            .interactions
            .acked(self.ix.telemetry_kind(), self.ix.command_name(), latency);
        let span = tracing::Span::current();
        spans::event(
            &span,
            SpanEvent::Acked,
            vec![opentelemetry::KeyValue::new("quack.interaction.ack", how)],
        );
        if outcome.is_done() {
            return Ok(());
        }
        tracing::warn!(ack = how, outcome = ?outcome, "Discord didn't take the acknowledgement");
        Err(Finish::Failed(match outcome {
            Outcome::RetryAfter(_) => Reason::RateLimited,
            Outcome::Unknown(_) => Reason::Timeout,
            _ => Reason::Discord,
        }))
    }

    const fn ephemeral(&self) -> bool {
        self.ix.ephemeral_allowed()
    }

    /// Defers a new message, ephemeral in a server (type 5).
    pub(crate) async fn defer(self) -> Result<Acked<'a>, Finish> {
        let reference = self.ix.reference();
        let outcome = self
            .shared
            .responder
            .defer(&reference, self.ephemeral())
            .await;
        self.acked(&outcome, "defer")?;
        Ok(self.into_acked())
    }

    /// Defers an update of the ephemeral message a component is on
    /// (type 6).
    pub(crate) async fn defer_update(self) -> Result<Acked<'a>, Finish> {
        let reference = self.ix.reference();
        let outcome = self.shared.responder.defer_update(&reference).await;
        self.acked(&outcome, "defer_update")?;
        Ok(self.into_acked())
    }

    /// Defers the way §11.3 asks for a component or modal from `source`:
    /// an update of an ephemeral message, else a new ephemeral message.
    pub(crate) async fn defer_from(self, source: Source) -> Result<Acked<'a>, Finish> {
        if source.ephemeral {
            self.defer_update().await
        } else {
            self.defer().await
        }
    }

    /// Answers at once with an ephemeral message (type 4).
    pub(crate) async fn reply(self, msg: &Msg, finish: Finish) -> Finish {
        let reference = self.ix.reference();
        let outcome = self
            .shared
            .responder
            .reply(&reference, msg, self.ephemeral())
            .await;
        match self.acked(&outcome, "reply") {
            Ok(()) => finish,
            Err(failed) => failed,
        }
    }

    /// Answers at once with a refusal or failure.
    pub(crate) async fn fail(self, failure: Failure) -> Finish {
        let msg = failure.msg();
        self.reply(&msg, failure.finish).await
    }

    /// Opens a modal (type 9) built from the custom id, caches, or static
    /// text.
    pub(crate) async fn modal(self, modal: &Modal) -> Finish {
        let reference = self.ix.reference();
        let outcome = self.shared.responder.modal(&reference, modal).await;
        match self.acked(&outcome, "modal") {
            Ok(()) => Finish::Done,
            Err(failed) => failed,
        }
    }

    /// Answers an autocomplete request (type 8).
    pub(crate) async fn choices(self, choices: &[AutocompleteChoice]) -> Finish {
        let reference = self.ix.reference();
        let outcome = self
            .shared
            .responder
            .autocomplete(&reference, choices)
            .await;
        match self.acked(&outcome, "autocomplete") {
            Ok(()) => Finish::Done,
            Err(failed) => failed,
        }
    }

    /// The documented exception (§11.3): authorize and read the one row a
    /// prefilled modal shows within [`PREFILL_DEADLINE`], then open the
    /// modal; a refusal is answered privately, and a missed deadline with
    /// `interaction.busy`.
    pub(crate) async fn prefilled_modal<F, Fut>(self, read: F) -> Finish
    where
        F: FnOnce(&'a Engine) -> Fut,
        Fut: Future<Output = Result<Modal, Failure>>,
    {
        let engine = &self.shared.engine;
        match tokio::time::timeout(PREFILL_DEADLINE, read(engine)).await {
            Ok(Ok(modal)) => self.modal(&modal).await,
            Ok(Err(failure)) => self.fail(failure).await,
            Err(_) => {
                self.fail(Failure::failed(quack_text::error::busy(), Reason::Timeout))
                    .await
            }
        }
    }

    fn into_acked(self) -> Acked<'a> {
        Acked {
            ix: self.ix,
            shared: self.shared,
        }
    }
}

/// An acknowledged interaction: the engine is available, and the answer
/// goes into the original response.
pub(crate) struct Acked<'a> {
    /// The interaction.
    pub(crate) ix: &'a Interaction,
    shared: &'a Shared,
}

impl<'a> Acked<'a> {
    /// The engine.
    pub(crate) fn engine(&self) -> &'a Engine {
        &self.shared.engine
    }

    /// The bot's shared state (modules).
    pub(crate) const fn shared(&self) -> &'a Shared {
        self.shared
    }

    /// Edits the original response (or the component's ephemeral message)
    /// to `msg`.
    pub(crate) async fn answer(&self, msg: &Msg) -> Finish {
        let outcome = self
            .shared
            .responder
            .edit_original(&self.ix.token, msg)
            .await;
        spans::event(&tracing::Span::current(), SpanEvent::Replied, Vec::new());
        if outcome.is_done() {
            Finish::Done
        } else {
            tracing::warn!(outcome = ?outcome, "couldn't edit the interaction's response");
            Finish::Failed(Reason::Discord)
        }
    }

    /// Answers with `result`: the message, or the failure's.
    pub(crate) async fn finish(&self, result: Result<Msg, Failure>) -> Finish {
        match result {
            Ok(msg) => self.answer(&msg).await,
            Err(failure) => {
                let replied = self.answer(&failure.msg()).await;
                if replied == Finish::Done {
                    failure.finish
                } else {
                    replied
                }
            }
        }
    }

    /// Answers with `result` when there is one; `Ok(None)` means another
    /// writer (a receipt's sync job) owns the original response.
    pub(crate) async fn finish_opt(&self, result: Result<Option<Msg>, Failure>) -> Finish {
        match result {
            Ok(None) => Finish::Done,
            Ok(Some(msg)) => self.answer(&msg).await,
            Err(failure) => self.finish(Err(failure)).await,
        }
    }
}
