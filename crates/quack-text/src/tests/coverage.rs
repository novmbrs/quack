//! Every message id of the design's catalog (§12) has a sample, and every
//! API error code (§14.6) has a message.

use std::collections::BTreeSet;

use quack_core::DomainError;

use super::samples::{all, api_codes, domain_errors};

const DESIGN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/design.md"));

fn section(start: &str, end: &str) -> &'static str {
    let from = DESIGN.find(start).unwrap_or(0);
    let to = DESIGN[from..].find(end).map_or(DESIGN.len(), |i| from + i);
    &DESIGN[from..to]
}

/// Backticked dotted ids in the catalog: `case.card.heading`.
fn catalog_ids() -> BTreeSet<String> {
    let text = section("## 12. Message catalog", "## 13. Settings");
    let mut ids = BTreeSet::new();
    for chunk in text.split('`').skip(1).step_by(2) {
        let ok = chunk.contains('.')
            && !chunk.contains("::")
            && chunk
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
            && !chunk.starts_with('.')
            && !chunk.ends_with('.');
        if ok {
            ids.insert(chunk.to_owned());
        }
    }
    ids
}

#[test]
fn every_catalog_id_has_a_sample() {
    let mut covered = BTreeSet::new();
    for s in all() {
        covered.insert(s.id.to_owned());
        covered.extend(s.covers.iter().map(|c| (*c).to_owned()));
    }
    let missing: Vec<String> = catalog_ids().difference(&covered).cloned().collect();
    assert!(
        missing.is_empty(),
        "catalog ids without a sample: {missing:#?}"
    );
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "matches what iter().map passes"
)]
const fn domain_index(e: &DomainError) -> usize {
    match e {
        DomainError::MissingCapability(_) => 0,
        DomainError::MfaRequired => 1,
        DomainError::MemberRefused(_) => 2,
        DomainError::CaseNotFound => 3,
        DomainError::CaseAlreadyVoided => 4,
        DomainError::CaseVoided => 5,
        DomainError::ReplacementInvalid => 6,
        DomainError::RuleNotFound => 7,
        DomainError::RuleArchived => 8,
        DomainError::RuleChanged => 9,
        DomainError::RuleNameTaken => 10,
        DomainError::TooManyRules => 11,
        DomainError::RuleUsedByHoneypot => 12,
        DomainError::RequiredFieldOnHoneypotRule => 13,
        DomainError::ActionNotRetryable => 14,
        DomainError::ActionNotDismissable => 15,
        DomainError::ActionNotUndoable => 16,
        DomainError::TimeoutWindowPassed => 17,
        DomainError::AppealNotFound => 18,
        DomainError::AppealChanged => 19,
        DomainError::AppealExists => 20,
        DomainError::AppealNotAllowed => 21,
        DomainError::EvidenceLimitReached { .. } => 22,
        DomainError::TooManyLinks => 23,
        DomainError::SettingsChanged => 24,
        DomainError::TicketNotOpen => 25,
        DomainError::TooManyFiles => 26,
        DomainError::EvidenceFileLimitReached { .. } => 27,
        DomainError::EvidenceChannelMissing => 28,
        DomainError::EvidenceChannelNotStaffOnly => 29,
        DomainError::EvidenceNotFound => 30,
        DomainError::EvidenceRemoved => 31,
        DomainError::EvidenceNotUpload => 32,
        DomainError::NoteChanged => 33,
        DomainError::UploadsBusy => 34,
    }
}

#[test]
fn every_domain_error_is_sampled() {
    let seen: BTreeSet<usize> = domain_errors().iter().map(domain_index).collect();
    assert_eq!(
        seen.len(),
        35,
        "add the new DomainError variant to the samples"
    );
}

#[test]
fn every_api_error_code_has_a_message() {
    let text = section("### 14.6 Errors", "### 14.7");
    let mut codes = BTreeSet::new();
    for line in text
        .lines()
        .filter(|l| l.starts_with("| 4") || l.starts_with("| 5"))
    {
        if let Some(code) = line.split('`').nth(1) {
            codes.insert(code.to_owned());
        }
    }
    assert!(codes.len() > 30, "parsed too few codes: {codes:?}");
    let mut known: BTreeSet<String> = api_codes().iter().map(|c| c.code().to_owned()).collect();
    known.extend(domain_errors().iter().map(|e| e.code().to_owned()));
    let missing: Vec<&String> = codes.iter().filter(|c| !known.contains(*c)).collect();
    assert!(
        missing.is_empty(),
        "API codes without a message: {missing:?}"
    );
}
