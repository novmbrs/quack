//! The error feed's tap (design §21.7): every `error` event, copied into a
//! small in-memory buffer that `quack-ops` drains each minute.
//!
//! Events from `quack_ops` itself are skipped, so a failing ops post can't
//! feed the feed. The buffer holds [`CAPACITY`] events; past that, events
//! are counted and dropped.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};

use opentelemetry::trace::TraceContextExt as _;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// The most events the buffer keeps between drains.
pub const CAPACITY: usize = 256;

/// The longest message kept per event.
const MESSAGE_MAX: usize = 300;

/// Targets the tap ignores (the ops crate posts the feed).
const SKIPPED: &str = "quack_ops";

/// One `error` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorEvent {
    /// The event's target (its module path).
    pub target: String,
    /// The message, then its other fields as `key=value`.
    pub message: String,
    /// The trace it happened in, when telemetry export is on.
    pub trace_id: Option<String>,
}

/// What [`drain`] returns: the events since the last drain and how many
/// did not fit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// The events, oldest first.
    pub events: Vec<ErrorEvent>,
    /// Events dropped because the buffer was full.
    pub dropped: u64,
}

#[derive(Default)]
struct Buffer {
    events: VecDeque<ErrorEvent>,
    dropped: u64,
}

static BUFFER: Mutex<Buffer> = Mutex::new(Buffer {
    events: VecDeque::new(),
    dropped: 0,
});

/// Takes every buffered event.
pub fn drain() -> Drained {
    let mut buffer = BUFFER.lock().unwrap_or_else(PoisonError::into_inner);
    Drained {
        events: buffer.events.drain(..).collect(),
        dropped: std::mem::take(&mut buffer.dropped),
    }
}

/// Adds one event (the layer's path; public for tests of the feed).
pub fn push(event: ErrorEvent) {
    let mut buffer = BUFFER.lock().unwrap_or_else(PoisonError::into_inner);
    if buffer.events.len() >= CAPACITY {
        buffer.dropped += 1;
    } else {
        buffer.events.push_back(event);
    }
}

/// The layer that copies `error` events into the buffer.
pub(crate) struct ErrorTap;

impl<S: Subscriber> Layer<S> for ErrorTap {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() != Level::ERROR || meta.target().starts_with(SKIPPED) {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let mut message = fields.message;
        if !fields.rest.is_empty() {
            if !message.is_empty() {
                message.push(' ');
            }
            message.push_str(&fields.rest);
        }
        push(ErrorEvent {
            target: meta.target().to_owned(),
            message: message.chars().take(MESSAGE_MAX).collect(),
            trace_id: trace_id(),
        });
    }
}

/// The current span's trace id, when it has a valid one.
fn trace_id() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            let _ = write!(self.rest, "{}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            if !self.rest.is_empty() {
                self.rest.push(' ');
            }
            let _ = write!(self.rest, "{}={value}", field.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

    /// The buffer is process-wide: one test at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn copies_error_events_but_not_others_or_its_own() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let subscriber = tracing_subscriber::registry().with(ErrorTap);
        tracing::subscriber::with_default(subscriber, || {
            drain();
            tracing::error!(guild_id = 7, "couldn't do the thing");
            tracing::warn!("only a warning");
            tracing::error!(target: "quack_ops::post", "the feed's own failure");
        });
        let drained = drain();
        assert_eq!(drained.events.len(), 1);
        let event = &drained.events[0];
        assert_eq!(event.message, "couldn't do the thing guild_id=7");
        assert!(event.target.starts_with("quack_telemetry"));
        assert_eq!(event.trace_id, None);
    }

    #[test]
    fn counts_what_does_not_fit() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        drain();
        for _ in 0..CAPACITY + 3 {
            push(ErrorEvent {
                target: "t".to_owned(),
                message: "m".to_owned(),
                trace_id: None,
            });
        }
        let drained = drain();
        assert_eq!(drained.events.len(), CAPACITY);
        assert_eq!(drained.dropped, 3);
    }
}
