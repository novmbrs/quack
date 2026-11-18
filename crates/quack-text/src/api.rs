//! API-only messages (§12.14) and the message for each transport-level
//! error code (§14.6). Domain refusals go through
//! [`crate::error::for_domain`].

use quack_core::modules::ModuleName;

use crate::fmt;
use crate::model::Text;
use crate::setup::module_name;

/// An API error that is not a domain refusal: authentication, request
/// shape, limits, and availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiCode {
    /// `400 validation_failed`.
    ValidationFailed,
    /// `400 invalid_json`.
    InvalidJson,
    /// `401 sign_in_required`.
    SignInRequired,
    /// `401 session_expired`.
    SessionExpired,
    /// `403 origin_rejected`.
    OriginRejected,
    /// `403 csrf_rejected`.
    CsrfRejected,
    /// `404 not_found`.
    NotFound,
    /// `404 action_not_found`.
    ActionNotFound,
    /// `404 server_not_found`.
    ServerNotFound,
    /// `404 file_unavailable`.
    FileUnavailable,
    /// `409 idempotency_key_reused`.
    IdempotencyKeyReused,
    /// `409 context_changed`.
    ContextChanged,
    /// `413 body_too_large`.
    BodyTooLarge,
    /// `415 unsupported_media_type`.
    UnsupportedMediaType,
    /// `428 idempotency_key_required`.
    IdempotencyKeyRequired,
    /// `429 rate_limited`, with the seconds to wait.
    RateLimited(u64),
    /// `409 module_not_ready`, for this module.
    ModuleNotReady(ModuleName),
    /// `502 discord_unavailable`.
    DiscordUnavailable,
    /// `503 unavailable`.
    Unavailable,
    /// `500 internal_error`.
    InternalError,
}

impl ApiCode {
    /// The machine code in the error envelope.
    pub const fn code(self) -> &'static str {
        match self {
            Self::ValidationFailed => "validation_failed",
            Self::InvalidJson => "invalid_json",
            Self::SignInRequired => "sign_in_required",
            Self::SessionExpired => "session_expired",
            Self::OriginRejected => "origin_rejected",
            Self::CsrfRejected => "csrf_rejected",
            Self::NotFound => "not_found",
            Self::ActionNotFound => "action_not_found",
            Self::ServerNotFound => "server_not_found",
            Self::FileUnavailable => "file_unavailable",
            Self::IdempotencyKeyReused => "idempotency_key_reused",
            Self::ContextChanged => "context_changed",
            Self::BodyTooLarge => "body_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::IdempotencyKeyRequired => "idempotency_key_required",
            Self::RateLimited(_) => "rate_limited",
            Self::ModuleNotReady(_) => "module_not_ready",
            Self::DiscordUnavailable => "discord_unavailable",
            Self::Unavailable => "unavailable",
            Self::InternalError => "internal_error",
        }
    }

    /// The message for this code.
    pub fn message(self) -> Text {
        match self {
            Self::ValidationFailed => validation_failed(),
            Self::InvalidJson | Self::UnsupportedMediaType => invalid_json(),
            Self::SignInRequired => sign_in_required(),
            Self::SessionExpired => session_expired(),
            Self::OriginRejected => origin_rejected(),
            Self::CsrfRejected => csrf_rejected(),
            Self::NotFound => not_found(),
            Self::ActionNotFound => crate::error::action_not_found(),
            Self::ServerNotFound => server_not_found(),
            Self::FileUnavailable => file_unavailable(),
            Self::IdempotencyKeyReused => idempotency_key_reused(),
            Self::ContextChanged => crate::error::context_changed(),
            Self::BodyTooLarge => body_too_large(),
            Self::IdempotencyKeyRequired => idempotency_key_required(),
            Self::RateLimited(seconds) => rate_limited(seconds),
            Self::ModuleNotReady(m) => module_not_ready(m),
            Self::DiscordUnavailable => crate::error::discord_unavailable(),
            Self::Unavailable => unavailable(),
            Self::InternalError => internal_error(),
        }
    }
}

/// `api.sign_in_required`.
pub fn sign_in_required() -> Text {
    Text::fixed("Sign in with Discord to continue.")
}

/// `api.session_expired`.
pub fn session_expired() -> Text {
    Text::fixed("Your session has ended. Sign in again.")
}

/// `api.origin_rejected`.
pub fn origin_rejected() -> Text {
    Text::fixed(
        "Quack only accepts changes made from its own dashboard. Open the dashboard and try again there.",
    )
}

/// `api.csrf_rejected`.
pub fn csrf_rejected() -> Text {
    Text::fixed("Quack couldn't verify this came from your session. Reload the page and try again.")
}

/// `api.not_found`.
pub fn not_found() -> Text {
    Text::fixed("Quack couldn't find that.")
}

/// `api.server_not_found`.
pub fn server_not_found() -> Text {
    Text::fixed("Quack isn't in that server, or you're not a member of it.")
}

/// `api.file_unavailable`.
pub fn file_unavailable() -> Text {
    Text::fixed("That file isn't available anymore.")
}

/// `api.validation_failed`.
pub fn validation_failed() -> Text {
    Text::fixed("Some fields need fixing.")
}

/// `api.invalid_json` (also `api.unsupported_media_type`, same words).
pub fn invalid_json() -> Text {
    Text::fixed("Quack couldn't read what was sent. Reload the dashboard and try again.")
}

/// `api.body_too_large`.
pub fn body_too_large() -> Text {
    Text::fixed("That's too much to save at once. The limit is 256 KB.")
}

/// `api.idempotency_key_required`.
pub fn idempotency_key_required() -> Text {
    Text::fixed(
        "Quack couldn't tell whether this was already saved. Reload the dashboard and try again.",
    )
}

/// `api.idempotency_key_reused`.
pub fn idempotency_key_reused() -> Text {
    Text::fixed(
        "This looks like a repeat of an earlier, different change. Reload the dashboard and try again.",
    )
}

/// `api.rate_limited`.
pub fn rate_limited(seconds: u64) -> Text {
    Text::fixed(format!(
        "You're doing that too often. Try again in {}.",
        fmt::plural(seconds.max(1), "second", "seconds")
    ))
}

/// `api.rule_changed`: someone else saved the rule being edited.
pub fn rule_changed() -> Text {
    Text::fixed(
        "Someone else saved this rule. Reload it to see the latest version before you save.",
    )
}

/// `api.rule_name_taken`.
pub fn rule_name_taken(name: &str) -> Text {
    Text::fixed("Another active rule is already called ")
        .bold_user(name)
        .s(". Choose a different name.")
}

/// `api.rule_used_by_honeypot`.
pub fn rule_used_by_honeypot() -> Text {
    Text::fixed(
        "The honeypot uses this rule, so it can't be archived. Choose another rule for the honeypot first.",
    )
}

/// `api.required_field_on_honeypot_rule`.
pub fn required_field_on_honeypot_rule() -> Text {
    Text::fixed(
        "The honeypot uses this rule, so its fields can't be required. Make the fields optional or choose another rule for the honeypot.",
    )
}

/// `api.too_many_rules`.
pub fn too_many_rules() -> Text {
    Text::fixed("A server can have at most 100 active rules. Archive one first.")
}

/// `api.settings_changed`.
pub fn settings_changed() -> Text {
    Text::fixed("Someone else changed the staff roles. Reload the settings and try again.")
}

/// `api.appeal_exists`.
pub fn appeal_exists() -> Text {
    Text::fixed("You already appealed this case.")
}

/// `api.appeal_not_allowed`.
pub fn appeal_not_allowed() -> Text {
    Text::fixed("This case can't be appealed.")
}

/// `api.module_not_ready`.
pub fn module_not_ready(module: ModuleName) -> Text {
    Text::fixed(format!(
        "{} can't be turned on yet.",
        fmt::upper_first(module_name(module))
    ))
}

/// `api.unavailable`.
pub fn unavailable() -> Text {
    Text::fixed("Quack is having trouble right now, so nothing changed. Try again in a moment.")
}

/// `api.internal_error`.
pub fn internal_error() -> Text {
    Text::fixed(
        "Something went wrong inside Quack. If you changed something, check whether it was saved before you try again.",
    )
}

/// `api.sign_in_failed` (sign-in page, not an API error).
pub fn sign_in_failed() -> Text {
    Text::fixed("Discord sign-in didn't finish. Start again.")
}

/// `api.sign_in_denied` (sign-in page, not an API error).
pub fn sign_in_denied() -> Text {
    Text::fixed("You cancelled the Discord sign-in.")
}
