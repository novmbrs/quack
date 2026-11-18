//! The REST client: one method per Discord operation Quack uses, each
//! returning a typed [`Outcome`].
//!
//! Every call has a deadline (10 s, 30 s with files), runs in a
//! `{method} {url.template}` client span, records
//! `http.client.request.duration`, waits for Twilight's rate limiter at most
//! the budget (10 s) and otherwise answers `RetryAfter`, and is never
//! retried here: the caller's per-kind policy decides (§17.3).

mod channels;
mod commands;
mod download;
mod interactions;
mod members;
mod messages;
pub(crate) mod multipart;
mod outcome;
mod route;
mod transport;

#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use twilight_http_ratelimiting::RateLimiter;

pub use channels::{FIND_THREADS_MAX_PAGES, ThreadPage};
pub use commands::RegisteredCommand;
pub use download::DownloadPolicy;
pub use interactions::AutocompleteChoice;
pub use members::ACCOUNTS_MAX;
pub use messages::{Mentions, MessageBody};
pub use outcome::{NotSentCause, Outcome, Refusal, RefusalKind, UnknownCause};

use self::route::Route;
use self::transport::{Body, Request, Transport};

/// Discord's API base URL (version 10).
pub const DISCORD_API: &str = "https://discord.com/api/v10";

/// How the client is set up. [`ClientConfig::new`] has the design's
/// deadlines; tests point `api_base` at a local server.
#[derive(Clone)]
pub struct ClientConfig {
    /// The bot token, without the `Bot ` prefix. Never printed.
    pub token: quack_core::Secret,
    /// The application id (also the OAuth client id).
    pub application_id: u64,
    /// The API base URL, without a trailing slash.
    pub api_base: String,
    /// The deadline for one ordinary call.
    pub request_timeout: Duration,
    /// The deadline for a call that uploads or downloads files.
    pub file_timeout: Duration,
    /// The longest Quack waits for a rate-limit permit before answering
    /// `RetryAfter`.
    pub rate_limit_budget: Duration,
    /// Where attachments may be downloaded from.
    pub downloads: DownloadPolicy,
}

impl ClientConfig {
    /// The design's settings for this token and application.
    pub fn new(token: impl Into<String>, application_id: u64) -> Self {
        Self {
            token: quack_core::Secret::new(token.into()),
            application_id,
            api_base: DISCORD_API.to_owned(),
            request_timeout: Duration::from_secs(10),
            file_timeout: Duration::from_secs(30),
            rate_limit_budget: Duration::from_secs(10),
            downloads: DownloadPolicy::DISCORD_CDN,
        }
    }
}

impl std::fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientConfig")
            .field("application_id", &self.application_id)
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

/// Why the client could not be built.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The HTTP client could not be built (TLS setup).
    #[error("couldn't build the HTTP client")]
    Http(#[source] reqwest::Error),
}

/// Quack's Discord REST client. Cheap to clone; every clone shares one rate
/// limiter and connection pool.
#[derive(Clone, Debug)]
pub struct DiscordClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    transport: Transport,
    downloader: reqwest::Client,
    downloads: DownloadPolicy,
    file_timeout: Duration,
    application_id: u64,
}

impl DiscordClient {
    /// Builds the client. Must run inside a Tokio runtime (the rate limiter
    /// spawns its task).
    pub fn new(config: &ClientConfig) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(ClientError::Http)?;
        let downloader = download::client(config.downloads).map_err(ClientError::Http)?;
        Ok(Self {
            inner: Arc::new(Inner {
                transport: Transport {
                    http,
                    api_base: config.api_base.trim_end_matches('/').to_owned(),
                    authorization: quack_core::Secret::new(format!(
                        "Bot {}",
                        config.token.expose()
                    )),
                    limiter: RateLimiter::default(),
                    closed: AtomicBool::new(false),
                    request_timeout: config.request_timeout,
                    file_timeout: config.file_timeout,
                    rate_limit_budget: config.rate_limit_budget,
                },
                downloader,
                downloads: config.downloads,
                file_timeout: config.file_timeout,
                application_id: config.application_id,
            }),
        })
    }

    /// The application id this client registers commands and answers
    /// interactions for.
    pub fn application_id(&self) -> u64 {
        self.inner.application_id
    }

    /// Stops accepting new calls (§17.5 step 4): every later call answers
    /// `NotSent(ShuttingDown)`. Calls already in flight finish.
    pub fn close(&self) {
        self.inner.transport.closed.store(true, Ordering::Release);
    }

    async fn call(&self, request: Request<'_>) -> Outcome<Vec<u8>> {
        self.inner.transport.send(request).await
    }

    /// Sends a request and decodes a JSON answer.
    async fn json<T: DeserializeOwned>(&self, request: Request<'_>) -> Outcome<T> {
        self.call(request).await.and_then(|body| decode(&body))
    }

    /// Sends a request whose success has no body Quack needs (204, or a
    /// body it ignores).
    async fn unit(&self, request: Request<'_>) -> Outcome<()> {
        self.call(request).await.map(|_| ())
    }

    /// Sends a request whose success returns an object (a member, channel,
    /// or message) Quack doesn't keep: a success whose body isn't one is
    /// `Unknown(Malformed)`, never `Done` (§17.3).
    async fn object(&self, request: Request<'_>) -> Outcome<()> {
        self.json::<serde_json::Map<String, serde_json::Value>>(request)
            .await
            .map(|_| ())
    }
}

/// Decodes a success body; one Quack can't read is `Unknown(Malformed)`
/// because the call itself succeeded.
fn decode<T: DeserializeOwned>(body: &[u8]) -> Outcome<T> {
    match serde_json::from_slice(body) {
        Ok(value) => Outcome::Done(value),
        Err(error) => {
            // Only where and what kind: serde's message can quote values
            // from the body (message text, names).
            tracing::warn!(
                category = ?error.classify(),
                line = error.line(),
                column = error.column(),
                "couldn't read Discord's answer"
            );
            Outcome::Unknown(UnknownCause::Malformed)
        }
    }
}

/// A converted value, or `Unknown(Malformed)` when conversion failed.
fn converted<T>(value: Option<T>) -> Outcome<T> {
    value.map_or(Outcome::Unknown(UnknownCause::Malformed), Outcome::Done)
}

/// A JSON request body.
fn json_body(value: &impl Serialize) -> Body {
    match serde_json::to_vec(value) {
        Ok(bytes) => Body::Json(bytes),
        // Serializing Quack's own plain structs cannot fail; an empty body
        // would make Discord refuse rather than guess.
        Err(_) => Body::Json(b"{}".to_vec()),
    }
}

/// A request with a JSON body.
fn with_json<'a>(route: Route, value: &impl Serialize) -> Request<'a> {
    Request {
        body: json_body(value),
        ..Request::new(route)
    }
}
