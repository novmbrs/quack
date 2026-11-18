//! Discord OAuth (§14.4): the authorize URL, the code exchange (through
//! the `oauth2` crate over a small reqwest adapter), and the two account
//! reads the dashboard needs. Every call has a 10 s deadline, reads at most
//! 1 MiB, follows no redirects, and is traced as a Discord REST call
//! without its credentials.

use std::time::{Duration, Instant};

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet, EndpointSet,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use quack_telemetry::labels::{HttpMethod, RestOutcome, UrlTemplate};
use quack_telemetry::{metrics, spans};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::Instrument as _;

use crate::config::{ConfigError, DiscordOauthConfig, PublicUrl};

/// The deadline of each call to Discord.
const DEADLINE: Duration = Duration::from_secs(10);
/// The largest response body read.
const MAX_BODY: usize = 1 << 20;

type Client = BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Why a call to Discord's OAuth or account endpoints failed. Carries no
/// response text: callers only learn that sign-in failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OauthError {
    /// No response, a timeout, or an oversized body.
    #[error("Discord could not be reached")]
    Transport,
    /// Discord answered with a non-success status.
    #[error("Discord answered {0}")]
    Status(u16),
    /// The response was not the expected JSON.
    #[error("unexpected response")]
    Body,
}

/// What the code exchange returned.
#[derive(Clone)]
pub(crate) struct Grant {
    /// The access token (only ever stored sealed).
    pub access_token: String,
    /// How long Discord says it lasts.
    pub expires_in: Option<Duration>,
}

/// The signed-in Discord account (`GET /users/@me`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct DiscordUser {
    /// The account ID.
    pub id: String,
    /// The username.
    pub username: String,
    /// The display name.
    #[serde(default)]
    pub global_name: Option<String>,
    /// The avatar hash.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Whether 2FA is on (§2.2).
    #[serde(default)]
    pub mfa_enabled: bool,
}

/// One server of the account (`GET /users/@me/guilds`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct UserGuild {
    /// The server ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its icon hash.
    #[serde(default)]
    pub icon: Option<String>,
    /// The account owns it.
    #[serde(default)]
    pub owner: bool,
    /// The account's permission bits there, as a decimal string.
    #[serde(default)]
    pub permissions: Option<String>,
}

/// Discord's OAuth endpoints for one application.
#[derive(Clone)]
pub struct DiscordOauth {
    client: Client,
    http: reqwest::Client,
    api_base: String,
}

impl std::fmt::Debug for DiscordOauth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordOauth")
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

impl DiscordOauth {
    /// Builds the client for `config`, redirecting to `public`'s callback.
    pub fn new(config: &DiscordOauthConfig, public: &PublicUrl) -> Result<Self, ConfigError> {
        let bad = |url: &str| ConfigError::DiscordUrl(url.to_owned());
        let api_base = config.api_base.trim_end_matches('/').to_owned();
        let token_url = format!("{api_base}/oauth2/token");
        let client = BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_client_secret(ClientSecret::new(config.client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new(config.authorize_url.clone())
                    .map_err(|_| bad(&config.authorize_url))?,
            )
            .set_token_uri(TokenUrl::new(token_url.clone()).map_err(|_| bad(&token_url))?)
            .set_redirect_uri(
                RedirectUrl::new(public.callback()).map_err(|_| bad(&public.callback()))?,
            );
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(DEADLINE)
            .connect_timeout(DEADLINE)
            .build()
            .map_err(|_| bad(&api_base))?;
        Ok(Self {
            client,
            http,
            api_base,
        })
    }

    /// The authorize page for a sign-in with this state and PKCE verifier,
    /// scopes `identify guilds`.
    pub(crate) fn authorize_url(&self, state: &str, verifier: &str) -> String {
        let challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            verifier.to_owned(),
        ));
        let state = state.to_owned();
        let (url, _) = self
            .client
            .authorize_url(move || CsrfToken::new(state))
            .add_scope(Scope::new("identify".to_owned()))
            .add_scope(Scope::new("guilds".to_owned()))
            .set_pkce_challenge(challenge)
            .url();
        url.into()
    }

    /// Exchanges an authorization code.
    pub(crate) async fn exchange(&self, code: &str, verifier: &str) -> Result<Grant, OauthError> {
        let started = Instant::now();
        let route = UrlTemplate::new("/oauth2/token");
        let span = spans::discord(HttpMethod::Post, route);
        let send = OauthHttp(&self.http);
        let result = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(PkceCodeVerifier::new(verifier.to_owned()))
            .request_async(&send)
            .instrument(span.clone())
            .await;
        let (status, outcome) = match &result {
            Ok(_) => (Some(200), RestOutcome::Done),
            Err(oauth2::RequestTokenError::ServerResponse(_)) => (Some(400), RestOutcome::Refused),
            Err(_) => (None, RestOutcome::Unknown),
        };
        if let Some(status) = status {
            spans::record_status(&span, status);
        }
        metrics()
            .discord
            .request(HttpMethod::Post, route, status, outcome, started.elapsed());
        let token = result.map_err(|e| match e {
            oauth2::RequestTokenError::ServerResponse(_) => OauthError::Status(400),
            oauth2::RequestTokenError::Request(e) => e,
            oauth2::RequestTokenError::Parse(..) | oauth2::RequestTokenError::Other(_) => {
                OauthError::Body
            }
        })?;
        Ok(Grant {
            access_token: token.access_token().secret().clone(),
            expires_in: token.expires_in(),
        })
    }

    /// The signed-in account.
    pub(crate) async fn current_user(&self, access_token: &str) -> Result<DiscordUser, OauthError> {
        self.get("/users/@me", access_token).await
    }

    /// The account's servers.
    pub(crate) async fn user_guilds(
        &self,
        access_token: &str,
    ) -> Result<Vec<UserGuild>, OauthError> {
        self.get("/users/@me/guilds", access_token).await
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &'static str,
        access_token: &str,
    ) -> Result<T, OauthError> {
        let started = Instant::now();
        let route = UrlTemplate::new(path);
        let span = spans::discord(HttpMethod::Get, route);
        let result = async {
            let response = self
                .http
                .get(format!("{}{path}", self.api_base))
                .bearer_auth(access_token)
                .send()
                .await
                .map_err(|_| OauthError::Transport)?;
            let status = response.status().as_u16();
            let body = read_body(response).await?;
            if !(200..300).contains(&status) {
                return Err(OauthError::Status(status));
            }
            serde_json::from_slice(&body).map_err(|_| OauthError::Body)
        }
        .instrument(span.clone())
        .await;
        let (status, outcome) = match &result {
            Ok(_) => (Some(200), RestOutcome::Done),
            Err(OauthError::Status(s)) => (Some(*s), RestOutcome::Refused),
            Err(OauthError::Body) => (Some(200), RestOutcome::Unknown),
            Err(OauthError::Transport) => (None, RestOutcome::Unknown),
        };
        if let Some(status) = status {
            spans::record_status(&span, status);
        }
        metrics()
            .discord
            .request(HttpMethod::Get, route, status, outcome, started.elapsed());
        result
    }
}

/// The `oauth2` crate's HTTP client: one reqwest call, bounded body.
struct OauthHttp<'a>(&'a reqwest::Client);

impl<'c> oauth2::AsyncHttpClient<'c> for OauthHttp<'_> {
    type Error = OauthError;
    type Future = std::pin::Pin<
        Box<dyn Future<Output = Result<oauth2::HttpResponse, OauthError>> + Send + 'c>,
    >;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(send_oauth(self.0, request))
    }
}

async fn send_oauth(
    http: &reqwest::Client,
    request: oauth2::HttpRequest,
) -> Result<oauth2::HttpResponse, OauthError> {
    let (parts, body) = request.into_parts();
    let response = http
        .request(parts.method, parts.uri.to_string())
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| OauthError::Transport)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = read_body(response).await?;
    let mut out = oauth2::HttpResponse::new(body);
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    Ok(out)
}

/// Reads a response body, refusing more than [`MAX_BODY`] bytes.
async fn read_body(mut response: reqwest::Response) -> Result<Vec<u8>, OauthError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| OauthError::Transport)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(OauthError::Transport);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
