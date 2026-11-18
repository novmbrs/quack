//! §12.3 errors and refusals, §12.14 API messages, and the domain-error
//! mapping.

use quack_core::DomainError;
use quack_core::action::ActionKind;
use quack_core::modules::ModuleName;
use quack_core::permissions::{Capability, DiscordPermission, MemberRefusal};

use super::{Sample, text};
use crate::api::{self, ApiCode};
use crate::error::{self, ErrorFacts, Scene};
use crate::tests::fixtures::*;

const REFUSALS: [(&str, MemberRefusal); 12] = [
    (
        "refusal.quack_not_in_server",
        MemberRefusal::QuackNotInServer,
    ),
    (
        "refusal.actor_not_in_server",
        MemberRefusal::ActorNotInServer,
    ),
    (
        "refusal.member_not_in_server",
        MemberRefusal::MemberNotInServer,
    ),
    ("refusal.member_is_self", MemberRefusal::MemberIsSelf),
    ("refusal.member_is_bot", MemberRefusal::MemberIsBot),
    ("refusal.member_is_owner", MemberRefusal::MemberIsOwner),
    (
        "refusal.member_role_too_high",
        MemberRefusal::MemberRoleTooHigh,
    ),
    ("refusal.quack_role_too_low", MemberRefusal::QuackRoleTooLow),
    (
        "refusal.quack_missing_permission",
        MemberRefusal::QuackMissingPermission(DiscordPermission::ModerateMembers),
    ),
    (
        "refusal.quack_missing_permission",
        MemberRefusal::QuackMissingPermission(DiscordPermission::KickMembers),
    ),
    (
        "refusal.quack_missing_permission",
        MemberRefusal::QuackMissingPermission(DiscordPermission::BanMembers),
    ),
    (
        "refusal.member_is_administrator",
        MemberRefusal::MemberIsAdministrator,
    ),
];

pub fn domain_errors() -> Vec<DomainError> {
    vec![
        DomainError::MissingCapability(Capability::Moderate),
        DomainError::MfaRequired,
        DomainError::MemberRefused(MemberRefusal::MemberIsBot),
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
    ]
}

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    for (id, c) in [
        ("error.missing_capability.moderate", Capability::Moderate),
        (
            "error.missing_capability.manage_rules",
            Capability::ManageRules,
        ),
        (
            "error.missing_capability.manage_settings",
            Capability::ManageSettings,
        ),
        (
            "error.missing_capability.manage_staff_roles",
            Capability::ManageStaffRoles,
        ),
    ] {
        v.push(text(id, "", error::missing_capability(c)));
    }
    v.push(text("error.mfa_required", "", error::mfa_required()));
    v.push(text(
        "error.actor_not_in_server",
        "",
        error::actor_not_in_server(),
    ));
    v.push(text(
        "error.quack_not_in_server",
        "",
        error::quack_not_in_server(),
    ));
    v.push(text(
        "error.discord_unavailable",
        "",
        error::discord_unavailable(),
    ));
    v.push(text("error.internal", "", error::internal()));
    v.push(text("error.unconfirmed", "", error::unconfirmed()));
    v.push(text("error.render_failed", "", error::render_failed(12)));
    v.push(text("error.case_not_found", "", error::case_not_found(12)));
    v.push(text(
        "error.action_not_found",
        "",
        error::action_not_found(),
    ));
    v.push(text("error.context_changed", "", error::context_changed()));
    v.push(text("error.rule_changed", "", error::rule_changed("Spam")));
    v.push(text("error.rule_not_found", "", error::rule_not_found()));
    v.push(text(
        "error.rule_archived",
        "",
        error::rule_archived("Spam"),
    ));
    v.push(text(
        "error.case_already_voided",
        "",
        error::case_already_voided(12),
    ));
    for kind in ActionKind::ALL {
        let k = crate::word::action(*kind);
        v.push(text("error.case_voided", k, error::case_voided(12, *kind)));
        v.push(text(
            "error.action_not_retryable",
            k,
            error::action_not_retryable(*kind),
        ));
        v.push(text(
            "error.action_not_dismissable",
            k,
            error::action_not_dismissable(*kind),
        ));
        v.push(text(
            "error.action_not_undoable",
            k,
            error::action_not_undoable(*kind),
        ));
    }
    v.push(text(
        "error.timeout_window_passed",
        "",
        error::timeout_window_passed(&sam()),
    ));
    v.push(text(
        "error.appeal_not_found",
        "",
        error::appeal_not_found(),
    ));
    v.push(text("error.appeal_changed", "", error::appeal_changed()));
    v.push(text(
        "error.evidence_limit_reached",
        "",
        error::evidence_limit_reached(12),
    ));
    v.push(text("error.too_many_links", "", error::too_many_links()));
    v.push(text(
        "error.replacement_invalid",
        "",
        error::replacement_invalid(12),
    ));
    v.push(text(
        "error.honeypot_policy_changed",
        "",
        error::honeypot_policy_changed(),
    ));
    v.push(text("interaction.busy", "", error::busy()));
    v.push(text(
        "interaction.expired_button",
        "",
        error::expired_button(),
    ));
    v.push(text(
        "interaction.legacy_button",
        "",
        error::legacy_button(),
    ));

    for (id, r) in REFUSALS {
        let variant = format!("{r:?}");
        v.push(text(
            id,
            variant.clone(),
            error::refusal(r, Some(&sam()), None),
        ));
        v.push(text(
            id,
            format!("{variant}, no member known"),
            error::refusal(r, None, None),
        ));
    }
    v.push(text(
        "refusal.quack_missing_permission",
        "for a timeout level",
        error::refusal(
            MemberRefusal::QuackMissingPermission(DiscordPermission::ModerateMembers),
            Some(&sam()),
            Some(ActionKind::Timeout),
        ),
    ));

    let full = ErrorFacts {
        scene: Scene::Staff,
        case_number: Some(12),
        rule: Some("Spam"),
        action: Some(ActionKind::Ban),
        member: None,
        module: Some(ModuleName::Tickets),
    };
    let s = sam();
    for err in domain_errors() {
        let name = format!("{err:?}");
        for (scene_name, scene) in [
            ("staff", Scene::Staff),
            ("adding a case", Scene::AddingCase),
            ("editing a rule", Scene::EditingRule),
            ("member", Scene::Member),
        ] {
            let facts = ErrorFacts {
                scene,
                member: Some(&s),
                ..full
            };
            v.push(text(
                "error.for_domain",
                format!("{name} · {scene_name} · all facts"),
                error::for_domain(&err, &facts),
            ));
        }
        let bare = ErrorFacts::default();
        v.push(text(
            "error.for_domain",
            format!("{name} · no facts"),
            error::for_domain(&err, &bare),
        ));
    }

    v.push(text("api.sign_in_required", "", api::sign_in_required()));
    v.push(text("api.session_expired", "", api::session_expired()));
    v.push(text("api.origin_rejected", "", api::origin_rejected()));
    v.push(text("api.csrf_rejected", "", api::csrf_rejected()));
    v.push(text("api.not_found", "", api::not_found()));
    v.push(text("api.server_not_found", "", api::server_not_found()));
    v.push(text("api.file_unavailable", "", api::file_unavailable()));
    v.push(text("api.validation_failed", "", api::validation_failed()));
    v.push(text("api.invalid_json", "", api::invalid_json()));
    v.push(text("api.body_too_large", "", api::body_too_large()));
    v.push(text(
        "api.unsupported_media_type",
        "",
        ApiCode::UnsupportedMediaType.message(),
    ));
    v.push(text(
        "api.idempotency_key_required",
        "",
        api::idempotency_key_required(),
    ));
    v.push(text(
        "api.idempotency_key_reused",
        "",
        api::idempotency_key_reused(),
    ));
    v.push(text(
        "api.rate_limited",
        "12 seconds",
        api::rate_limited(12),
    ));
    v.push(text("api.rate_limited", "1 second", api::rate_limited(1)));
    v.push(text("api.rule_changed", "", api::rule_changed()));
    v.push(text(
        "api.rule_name_taken",
        "",
        api::rule_name_taken("Spam"),
    ));
    v.push(text(
        "api.rule_used_by_honeypot",
        "",
        api::rule_used_by_honeypot(),
    ));
    v.push(text(
        "api.required_field_on_honeypot_rule",
        "",
        api::required_field_on_honeypot_rule(),
    ));
    v.push(text("api.too_many_rules", "", api::too_many_rules()));
    v.push(text("api.settings_changed", "", api::settings_changed()));
    v.push(text("api.appeal_exists", "", api::appeal_exists()));
    v.push(text(
        "api.appeal_not_allowed",
        "",
        api::appeal_not_allowed(),
    ));
    for m in ModuleName::ALL {
        v.push(text(
            "api.module_not_ready",
            format!("{m:?}"),
            api::module_not_ready(*m),
        ));
    }
    v.push(text(
        "api.discord_unavailable",
        "",
        ApiCode::DiscordUnavailable.message(),
    ));
    v.push(text("api.unavailable", "", api::unavailable()));
    v.push(text("api.internal_error", "", api::internal_error()));
    v.push(text("api.sign_in_failed", "", api::sign_in_failed()));
    v.push(text("api.sign_in_denied", "", api::sign_in_denied()));
    for code in api_codes() {
        v.push(text("api.code", code.code(), code.message()));
    }
    v
}

pub fn api_codes() -> Vec<ApiCode> {
    vec![
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
        ApiCode::BodyTooLarge,
        ApiCode::UnsupportedMediaType,
        ApiCode::IdempotencyKeyRequired,
        ApiCode::RateLimited(30),
        ApiCode::ModuleNotReady(ModuleName::Honeypot),
        ApiCode::DiscordUnavailable,
        ApiCode::Unavailable,
        ApiCode::InternalError,
    ]
}
