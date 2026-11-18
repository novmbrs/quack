//! The OpenAPI 3.1 document (§14.12), generated from the handlers'
//! annotations. `contracts/http/openapi.json` is its committed copy; the
//! `openapi_is_current` test fails when they differ, and
//! `UPDATE_OPENAPI=1 cargo nextest run -p quack-api openapi_is_current`
//! rewrites it. The dashboard's `bun run api` turns it into TypeScript.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::openapi::{InfoBuilder, OpenApi, OpenApiBuilder};

/// The committed document's path, relative to the workspace root.
pub const DOCUMENT_PATH: &str = "contracts/http/openapi.json";

/// The document's frame: info and the two security schemes every
/// signed-in route uses.
pub(crate) fn base() -> OpenApi {
    let mut doc = OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("Quack")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(
                    "The API of Quack's dashboard. Same-origin only: sign in with Discord \
                     (session cookie); every POST, PUT, PATCH, and DELETE also sends \
                     `X-CSRF-Token` (from `GET /api/auth/me`) and, under `/api/guilds` and \
                     `/api/me`, an `Idempotency-Key`. Errors use one envelope, \
                     `{ \"error\": ErrorBody }`.",
                ))
                .build(),
        )
        .build();
    let components = doc.components.get_or_insert_with(Default::default);
    components.add_security_scheme(
        "session",
        SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
            "__Host-quack_session",
            "The session cookie (`quack_session` on a loopback http dashboard).",
        ))),
    );
    components.add_security_scheme(
        "csrf",
        SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
            "X-CSRF-Token",
            "The session's CSRF token, required on every mutation.",
        ))),
    );
    doc
}

/// The OpenAPI 3.1 document of every route.
pub fn document() -> OpenApi {
    let (_, doc) = crate::routes::api().split_for_parts();
    doc
}

/// The document as the committed JSON (pretty, trailing newline).
pub fn document_json() -> String {
    let mut json = document().to_pretty_json().unwrap_or_default();
    json.push('\n');
    json
}
