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
