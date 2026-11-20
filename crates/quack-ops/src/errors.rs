//! The error feed (§21.7): each minute, the `error` events the telemetry
//! tap buffered, grouped and posted to `#ops-errors`. Best effort: nothing
//! is retried, and what doesn't post is still in the logs.

use quack_engine::port::{Nonce, Outcome};
use quack_telemetry::errors::{Drained, ErrorEvent};
use quack_text::ops::{self as text, ErrorGroup};

use crate::Ops;

/// Groups events by target and message, in first-seen order.
pub(crate) fn group(events: Vec<ErrorEvent>, trace_url: Option<&str>) -> Vec<ErrorGroup> {
    let mut groups: Vec<ErrorGroup> = Vec::new();
    for e in events {
        if let Some(g) = groups
            .iter_mut()
            .find(|g| g.target == e.target && g.message == e.message)
        {
            g.count += 1;
            if g.trace_id.is_none() {
                g.trace_url = link(trace_url, e.trace_id.as_deref());
                g.trace_id = e.trace_id;
            }
        } else {
            groups.push(ErrorGroup {
                trace_url: link(trace_url, e.trace_id.as_deref()),
                target: e.target,
                message: e.message,
                count: 1,
                trace_id: e.trace_id,
            });
        }
    }
    groups
}

fn link(template: Option<&str>, trace_id: Option<&str>) -> Option<String> {
    Some(template?.replace("{trace_id}", trace_id?))
}

/// Posts the minute's errors, if there were any.
pub(crate) async fn flush(ops: &Ops) {
    let Drained { events, dropped } = quack_telemetry::errors::drain();
    if events.is_empty() && dropped == 0 {
        return;
    }
    let Some(channel) = ops.channels().and_then(|c| c.errors) else {
        return;
    };
    let groups = group(events, ops.config().trace_url.as_deref());
    let msg = text::errors(&groups, dropped);
    let nonce = Nonce::new(quack_core::ids::JobId::from_uuid(uuid::Uuid::now_v7()).to_u22());
    match ops
        .engine()
        .discord()
        .send(channel, &msg, &[], &nonce)
        .await
    {
        Outcome::Done(_) => {}
        other => tracing::warn!(outcome = %other.code(), "couldn't post the error feed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(message: &str, trace: Option<&str>) -> ErrorEvent {
        ErrorEvent {
            target: "quack_engine::x".to_owned(),
            message: message.to_owned(),
            trace_id: trace.map(str::to_owned),
        }
    }

    #[test]
    fn groups_repeats_and_keeps_the_first_trace() {
        let groups = group(
            vec![
                event("a", None),
                event("b", Some("t1")),
                event("a", Some("t2")),
                event("a", Some("t3")),
            ],
            Some("https://grafana.example/t/{trace_id}"),
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].message, "a");
        assert_eq!(groups[0].count, 3);
        assert_eq!(groups[0].trace_id.as_deref(), Some("t2"));
        assert_eq!(
            groups[0].trace_url.as_deref(),
            Some("https://grafana.example/t/t2")
        );
        assert_eq!(groups[1].count, 1);
    }

    #[test]
    fn no_template_means_no_link() {
        let groups = group(vec![event("a", Some("t1"))], None);
        assert_eq!(groups[0].trace_url, None);
        assert_eq!(groups[0].trace_id.as_deref(), Some("t1"));
    }
}
