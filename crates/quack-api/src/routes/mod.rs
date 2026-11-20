//! Every route of §14.2, one module per resource, and the route table that
//! gives each route its rate-limit class and telemetry name.

mod actions;
mod appeals;
mod auth;
mod cases;
pub(crate) use cases::{UPLOAD_BUDGET_BYTES, UploadBudget};
mod directory;
mod evidence;
mod guilds;
mod health;
mod me;
mod members;
mod mod_log;
pub(crate) mod modules;
mod rules;
mod settings;
mod statistics;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use utoipa_axum::router::OpenApiRouter;

use crate::error::ApiError;
use crate::middleware::rate_limit::RateClass;
use crate::state::ApiState;

use RateClass::{ActionControl, CaseCreate, MemberWrite, Read, SignIn, Write};

const GET: Method = Method::GET;
const POST: Method = Method::POST;
const PUT: Method = Method::PUT;
const PATCH: Method = Method::PATCH;

/// Route 60, the one route whose body may be larger than 256 KiB (§14.8).
pub(crate) const UPLOAD_ROUTE: &str = "/api/guilds/{guild_id}/cases/{case_ref}/evidence/uploads";

/// Every route with its rate class (§14.2, §14.7). A test checks it lists
/// exactly the OpenAPI document's operations.
static ROUTES: &[(Method, &str, RateClass)] = &[
    (GET, "/api/auth/login", SignIn),
    (GET, "/api/auth/callback", SignIn),
    (GET, "/api/auth/me", Read),
    (POST, "/api/auth/logout", Write),
    (GET, "/api/guilds", Read),
    (GET, "/api/guilds/{guild_id}/me", Read),
    (GET, "/api/guilds/{guild_id}/health", Read),
    (GET, "/api/guilds/{guild_id}/statistics", Read),
    (GET, "/api/guilds/{guild_id}/directory/members", Read),
    (GET, "/api/guilds/{guild_id}/directory/channels", Read),
    (GET, "/api/guilds/{guild_id}/directory/roles", Read),
    (GET, "/api/guilds/{guild_id}/rules", Read),
    (POST, "/api/guilds/{guild_id}/rules", Write),
    (GET, "/api/guilds/{guild_id}/rules/{rule_id}", Read),
    (PUT, "/api/guilds/{guild_id}/rules/{rule_id}", Write),
    (
        POST,
        "/api/guilds/{guild_id}/rules/{rule_id}/archive",
        Write,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/rules/{rule_id}/restore",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/rules/{rule_id}/export", Read),
    (POST, "/api/guilds/{guild_id}/rules/import", Write),
    (
        GET,
        "/api/guilds/{guild_id}/rules/{rule_id}/versions/{version}",
        Read,
    ),
    (GET, "/api/guilds/{guild_id}/cases", Read),
    (POST, "/api/guilds/{guild_id}/cases", CaseCreate),
    (GET, "/api/guilds/{guild_id}/cases/preview", Read),
    (GET, "/api/guilds/{guild_id}/cases/{case_ref}", Read),
    (POST, "/api/guilds/{guild_id}/cases/{case_ref}/void", Write),
    (
        PUT,
        "/api/guilds/{guild_id}/cases/{case_ref}/context",
        Write,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/cases/{case_ref}/evidence",
        Write,
    ),
    (
        GET,
        "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{file_id}",
        Read,
    ),
    (POST, UPLOAD_ROUTE, Write),
    (
        PUT,
        "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/note",
        Write,
    ),
    (
        PUT,
        "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/visibility",
        Write,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/cases/{case_ref}/evidence/{evidence_id}/remove",
        Write,
    ),
    (
        GET,
        "/api/guilds/{guild_id}/members/{member_id}/summary",
        Read,
    ),
    (GET, "/api/guilds/{guild_id}/actions", Read),
    (
        POST,
        "/api/guilds/{guild_id}/actions/{action_id}/retry",
        ActionControl,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/actions/{action_id}/dismiss",
        ActionControl,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/actions/{action_id}/undo",
        ActionControl,
    ),
    (GET, "/api/guilds/{guild_id}/appeals", Read),
    (GET, "/api/guilds/{guild_id}/appeals/{appeal_id}", Read),
    (
        POST,
        "/api/guilds/{guild_id}/appeals/{appeal_id}/decision",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/mod-log", Read),
    (GET, "/api/guilds/{guild_id}/settings", Read),
    (PATCH, "/api/guilds/{guild_id}/settings", Write),
    (
        POST,
        "/api/guilds/{guild_id}/settings/starter-notice/acknowledge",
        Write,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/modules/{module}/enable",
        Write,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/modules/{module}/disable",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/modules/tickets", Read),
    (
        PUT,
        "/api/guilds/{guild_id}/modules/tickets/settings",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/modules/activity-log", Read),
    (
        PUT,
        "/api/guilds/{guild_id}/modules/activity-log/settings",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/modules/honeypot", Read),
    (
        PUT,
        "/api/guilds/{guild_id}/modules/honeypot/settings",
        Write,
    ),
    (GET, "/api/guilds/{guild_id}/tickets", Read),
    (GET, "/api/guilds/{guild_id}/tickets/{ticket_id}", Read),
    (
        GET,
        "/api/guilds/{guild_id}/tickets/{ticket_id}/transcript",
        Read,
    ),
    (
        POST,
        "/api/guilds/{guild_id}/tickets/{ticket_id}/close",
        Write,
    ),
    (GET, "/api/me/cases", Read),
    (GET, "/api/me/cases/{case_id}", Read),
    (GET, "/api/me/cases/{case_id}/evidence/{file_id}", Read),
    (POST, "/api/me/cases/{case_id}/appeal", MemberWrite),
    (
        POST,
        "/api/me/cases/{case_id}/appeal/information",
        MemberWrite,
    ),
    (GET, "/livez", RateClass::None),
    (GET, "/readyz", RateClass::None),
];

/// The telemetry name of a request's route: its template, or a fixed name
/// when no route matched (never the raw path).
pub(crate) fn route_template(matched: Option<&str>, path: &str) -> &'static str {
    match matched {
        Some(m) => ROUTES
            .iter()
            .find(|(_, path, _)| *path == m)
            .map_or("unknown", |(_, path, _)| path),
        None if path == "/api" || path.starts_with("/api/") => "/api/{unmatched}",
        None => "/{dashboard}",
    }
}

/// The rate class of a matched route (`None` when unmatched or unlimited).
pub(crate) fn rate_class(method: &Method, matched: Option<&str>) -> RateClass {
    let method = if *method == Method::HEAD {
        &Method::GET
    } else {
        method
    };
    matched
        .and_then(|m| {
            ROUTES
                .iter()
                .find(|(verb, path, _)| *path == m && verb == method)
        })
        .map_or(RateClass::None, |(_, _, class)| *class)
}

/// Every API route as one OpenAPI router (the document and the app).
pub(crate) fn api() -> OpenApiRouter<ApiState> {
    OpenApiRouter::with_openapi(crate::openapi::base())
        .merge(auth::routes())
        .merge(guilds::routes())
        .merge(health::routes())
        .merge(statistics::routes())
        .merge(directory::routes())
        .merge(rules::routes())
        .merge(cases::routes())
        .merge(evidence::routes())
        .merge(members::routes())
        .merge(actions::routes())
        .merge(appeals::routes())
        .merge(mod_log::routes())
        .merge(settings::routes())
        .merge(modules::routes())
        .merge(me::routes())
}

/// The whole application.
pub(crate) fn app(state: ApiState) -> Router {
    let (router, _) = api().split_for_parts();
    let router = router
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed);
    crate::middleware::stack(router, &state).with_state(state)
}

/// No route matched: the API answers `404 not_found`; other paths are
/// the dashboard's files.
async fn fallback(State(state): State<ApiState>, request: Request) -> Response {
    let path = request.uri().path();
    if crate::middleware::headers::is_api_or_health(path) {
        return ApiError::not_found().into_response();
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    crate::static_files::serve(&state, request).await
}

/// A known API path with another method is `404 not_found` (§14.6).
async fn method_not_allowed() -> Response {
    ApiError::not_found().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_route_table_matches_the_document() {
        let doc = crate::openapi::document();
        let mut documented: Vec<(String, String)> = doc
            .paths
            .paths
            .iter()
            .flat_map(|(path, item)| {
                let json = serde_json::to_value(item).unwrap_or_default();
                json.as_object().map_or_else(Vec::new, |o| {
                    o.keys()
                        .filter(|k| {
                            matches!(k.as_str(), "get" | "post" | "put" | "patch" | "delete")
                        })
                        .map(|k| (k.to_uppercase(), path.clone()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        documented.sort();
        let mut table: Vec<(String, String)> = ROUTES
            .iter()
            .map(|(method, path, _)| (method.to_string(), (*path).to_owned()))
            .collect();
        table.sort();
        assert_eq!(documented, table);
    }
}
