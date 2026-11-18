//! The error envelope (§14.6) for every transport code and every domain
//! refusal: status, code, message, and extras, snapshotted.

use axum::response::IntoResponse;
use quack_api::ApiError;
use quack_core::DomainError;
use quack_core::action::ActionKind;
use quack_core::modules::ModuleName;
use quack_core::permissions::{Capability, DiscordPermission, MemberRefusal};
use quack_text::api::ApiCode;
use quack_text::error::{ErrorFacts, Scene};
use serde_json::{Value, json};

fn entry(error: &ApiError) -> Value {
    let envelope = serde_json::to_value(error.envelope()).unwrap();
    json!({"status": error.status().as_u16(), "envelope": envelope})
}

#[test]
fn every_api_code() {
    let codes = [
        ApiCode::ValidationFailed,
        ApiCode::InvalidJson,
        ApiCode::SignInRequired,
        ApiCode::SessionExpired,
        ApiCode::OriginRejected,
        ApiCode::CsrfRejected,
        ApiCode::NotFound,
        ApiCode::ActionNotFound,
        ApiCode::ServerNotFound,
        ApiCode::FileUnavailable,
        ApiCode::IdempotencyKeyReused,
        ApiCode::ContextChanged,
        ApiCode::CaseNumbersExhausted,
        ApiCode::BodyTooLarge,
        ApiCode::UnsupportedMediaType,
        ApiCode::IdempotencyKeyRequired,
        ApiCode::RateLimited(12),
        ApiCode::ModuleNotReady(ModuleName::Tickets),
        ApiCode::DiscordUnavailable,
        ApiCode::Unavailable,
        ApiCode::InternalError,
    ];
    let all: Vec<Value> = codes.iter().map(|c| entry(&ApiError::api(*c))).collect();
    insta::assert_json_snapshot!("api_codes", all);
}

#[test]
fn every_domain_refusal() {
    let errors = [
        DomainError::MissingCapability(Capability::Moderate),
        DomainError::MissingCapability(Capability::ManageRules),
        DomainError::MissingCapability(Capability::ManageSettings),
        DomainError::MissingCapability(Capability::ManageStaffRoles),
        DomainError::MfaRequired,
        DomainError::MemberRefused(MemberRefusal::MemberRoleTooHigh),
        DomainError::MemberRefused(MemberRefusal::QuackMissingPermission(
            DiscordPermission::BanMembers,
        )),
        DomainError::CaseNotFound,
        DomainError::CaseAlreadyVoided,
        DomainError::CaseVoided,
        DomainError::ReplacementInvalid,
        DomainError::RuleNotFound,
        DomainError::RuleArchived,
        DomainError::RuleChanged,
        DomainError::RuleNameTaken,
        DomainError::TooManyRules,
        DomainError::RuleUsedByHoneypot,
        DomainError::RequiredFieldOnHoneypotRule,
        DomainError::ActionNotRetryable,
        DomainError::ActionNotDismissable,
        DomainError::ActionNotUndoable,
        DomainError::TimeoutWindowPassed,
        DomainError::AppealNotFound,
        DomainError::AppealChanged,
        DomainError::AppealExists,
        DomainError::AppealNotAllowed,
        DomainError::EvidenceLimitReached { count: 50 },
        DomainError::TooManyLinks,
        DomainError::SettingsChanged,
        DomainError::TicketNotOpen,
    ];
    let staff = ErrorFacts {
        scene: Scene::Staff,
        case_number: Some(12),
        rule: Some("Spam"),
        action: Some(ActionKind::Ban),
        member: None,
    };
    let member = ErrorFacts {
        scene: Scene::Member,
        ..ErrorFacts::default()
    };
    let all: Vec<Value> = errors
        .iter()
        .map(|e| json!({"staff": entry(&ApiError::domain(e, &staff)), "member": entry(&ApiError::domain(e, &member))}))
        .collect();
    insta::assert_json_snapshot!("domain_refusals", all);
}

#[test]
fn rate_limited_sets_retry_after() {
    let response = ApiError::api(ApiCode::RateLimited(7)).into_response();
    assert_eq!(response.status(), 429);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("7")
    );
}
