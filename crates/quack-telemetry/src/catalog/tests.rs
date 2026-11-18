use std::collections::HashSet;

use super::*;

const SEMCONV_PREFIXES: &[&str] = &["http.", "db.", "process."];

#[test]
fn names_are_unique_and_well_formed() {
    let mut seen = HashSet::new();
    for spec in INSTRUMENTS {
        assert!(seen.insert(spec.name), "{} is declared twice", spec.name);
        assert!(
            spec.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
            "{} must be lowercase and dot-separated",
            spec.name
        );
        assert!(
            spec.name.starts_with("quack.")
                || SEMCONV_PREFIXES.iter().any(|p| spec.name.starts_with(p)),
            "{} must start with quack. or be a semantic-convention name",
            spec.name
        );
        assert!(!spec.unit.is_empty(), "{} has no unit", spec.name);
        assert!(
            !spec.description.is_empty(),
            "{} has no description",
            spec.name
        );
    }
}

#[test]
fn histograms_and_only_histograms_have_increasing_buckets() {
    for spec in INSTRUMENTS {
        if spec.kind == InstrumentKind::Histogram {
            assert!(!spec.buckets.is_empty(), "{} needs buckets", spec.name);
            assert!(
                spec.buckets.windows(2).all(|pair| pair[0] < pair[1]),
                "{} buckets must increase",
                spec.name
            );
        } else {
            assert!(spec.buckets.is_empty(), "{} is not a histogram", spec.name);
        }
    }
}

#[test]
fn every_attribute_is_documented_and_used() {
    let documented: HashSet<_> = ATTRIBUTES.iter().map(|a| a.key).collect();
    assert_eq!(
        documented.len(),
        ATTRIBUTES.len(),
        "an attribute is documented twice"
    );
    let used: HashSet<_> = INSTRUMENTS
        .iter()
        .flat_map(|spec| spec.attributes.iter().copied())
        .collect();
    let undocumented: Vec<_> = used.difference(&documented).collect();
    let unused: Vec<_> = documented.difference(&used).collect();
    assert!(
        undocumented.is_empty(),
        "undocumented attributes: {undocumented:?}"
    );
    assert!(
        unused.is_empty(),
        "documented but unused attributes: {unused:?}"
    );
}

#[test]
fn ack_latency_has_fine_buckets_below_three_seconds() {
    let below = INTERACTION_ACK_DURATION
        .buckets
        .iter()
        .filter(|&&bound| bound <= 3.0)
        .count();
    assert!(below >= 15, "only {below} ack buckets at or below 3 s");
}

/// `METRICS.md` is generated from this table; regenerate it with
/// `UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry metrics_doc`.
#[test]
fn metrics_doc_matches_the_catalog() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("METRICS.md");
    let rendered = render_markdown();
    if std::env::var_os("UPDATE_METRICS_DOC").is_some() {
        std::fs::write(&path, &rendered).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == rendered,
        "METRICS.md is out of date; run UPDATE_METRICS_DOC=1 cargo test -p quack-telemetry metrics_doc"
    );
}

const DESIGN: &str = include_str!("../../../../docs/design.md");

/// Span-only keys the design names; they never go on metrics.
const SPAN_KEYS: &[&str] = &[
    crate::keys::GUILD_ID,
    crate::keys::ACTOR_ID,
    crate::keys::INTERACTION_ID,
    crate::keys::JOB_ID,
    crate::keys::JOB_ATTEMPT,
    crate::keys::JOB_MAX_ATTEMPTS,
    crate::keys::JOB_QUEUE_WAIT,
    "db.query.summary",
];

/// Every `quack.*`, `http.*`, `db.*`, and `process.*` name in backticks in
/// the design.
fn design_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    for chunk in DESIGN.split('`').skip(1).step_by(2) {
        let head = chunk.split(['{', '=', ' ']).next().unwrap_or_default();
        if ["quack.", "http.", "db.", "process."]
            .iter()
            .any(|p| head.starts_with(p))
            && head
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._*".contains(c))
        {
            names.push(head);
        }
    }
    names
}

#[test]
fn every_metric_name_in_the_design_is_in_the_catalog() {
    let names = design_names();
    assert!(names.len() > 50, "found only {names:?}");
    for name in names {
        let known = find(name).is_some()
            || ATTRIBUTES.iter().any(|a| a.key == name)
            || SPAN_KEYS.contains(&name)
            || name
                .strip_suffix('*')
                .is_some_and(|prefix| INSTRUMENTS.iter().any(|s| s.name.starts_with(prefix)));
        assert!(
            known,
            "the design names `{name}`, which the crate does not define"
        );
    }
    for new in [
        "quack.job.lease_expired",
        "quack.jobs.dead",
        "quack.actions.finished",
        "quack.cases.refused",
    ] {
        assert!(find(new).is_some(), "{new}");
    }
}

#[test]
fn attribute_values_in_the_design_are_allowed() {
    // `instrument{key="value"}` filters in the alert table and elsewhere.
    for chunk in DESIGN.split('`').skip(1).step_by(2) {
        let Some((_, filters)) = chunk.split_once('{') else {
            continue;
        };
        for filter in filters.trim_end_matches('}').split(',') {
            let Some((key, value)) = filter.split_once('=') else {
                continue;
            };
            let value = value.trim_matches('"');
            if let Some(attribute) = ATTRIBUTES.iter().find(|a| a.key == key)
                && !attribute.values.is_empty()
            {
                assert!(
                    attribute.values.contains(&value),
                    "the design filters `{key}` on {value:?}, which is not one of {:?}",
                    attribute.values
                );
            }
        }
    }
}

#[test]
fn design_instrument_attributes_match_the_catalog() {
    // `instrument{a,b}` attribute lists the design gives next to metrics.
    for chunk in DESIGN.split('`').skip(1).step_by(2) {
        let Some((name, rest)) = chunk.split_once('{') else {
            continue;
        };
        let Some(spec) = find(name) else {
            continue;
        };
        for key in rest.trim_end_matches('}').split(',') {
            let key = key.split('=').next().unwrap_or_default();
            assert!(
                spec.attributes.contains(&key),
                "the design records `{name}` with `{key}`, which the catalog does not allow"
            );
        }
    }
}
