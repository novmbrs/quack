//! Quack's HTTP API and dashboard host.
//!
//! The axum API under `/api`, Discord OAuth sign-in, PostgreSQL-backed
//! sessions, CSRF protection, in-process rate limits, the OpenAPI 3.1
//! document, and the dashboard's built files, all on one origin (§14).
//!
//! The binary builds an [`ApiState`] once, turns it into a router with
//! [`router`], and serves it with [`serve`], which drains in-flight
//! requests when its shutdown future completes:
//!
//! ```ignore
//! let state = ApiState::new(engine, config, readiness)?;
//! let app = quack_api::router(state);
//! quack_api::serve(listener, app, shutdown.cancelled_owned()).await?;
//! ```
//!
//! Every handler resolves the signed-in account from its session, builds
//! the staff principal from current Discord facts through the engine,
//! checks the route's capability (before any operation-key replay), calls
//! one engine use case, and maps its result to a wire type. Errors leave
//! through [`ApiError`] only, so every failure has the §14.6 envelope.

mod auth;
mod config;
mod error;
mod extract;
mod middleware;
pub mod openapi;
mod pagination;
mod readiness;
mod routes;
mod serve;
mod state;
mod static_files;
mod wire;

pub use auth::oauth::DiscordOauth;
pub use config::{ApiConfig, ConfigError, DiscordOauthConfig, PublicUrl, SessionSecret};
pub use error::{ApiError, ErrorBody, ErrorEnvelope, FieldError, Problem};
pub use extract::{
    IdempotencyKey, InServer, ManageRules, ManageSettings, MemberSelf, Moderate, ReadRules,
    Requirement, SessionAccount, Staff,
};
pub use readiness::{Readiness, ReadinessFacts};
pub use routes::modules::ModuleServices;
pub use serve::serve;
pub use state::ApiState;

/// Builds the whole HTTP application: the API, health endpoints, and the
/// dashboard's files, with every middleware layer in the designed order.
pub fn router(state: ApiState) -> axum::Router {
    routes::app(state)
}
