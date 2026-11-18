//! What the API needs from the binary's configuration (§17.7), validated
//! once when it is built.

use std::fmt;
use std::path::PathBuf;

use url::Url;

/// Why the API configuration was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// `QUACK_PUBLIC_URL` is not an absolute `https` URL (or `http` on a
    /// loopback host) without credentials, path, query, or fragment.
    #[error("QUACK_PUBLIC_URL must be an https origin (http only for localhost): {0}")]
    PublicUrl(String),
    /// `QUACK_SESSION_SECRET` is not base64 of exactly 32 bytes.
    #[error("QUACK_SESSION_SECRET must be base64 of exactly 32 bytes")]
    SessionSecret,
    /// `QUACK_TRUSTED_PROXY_HOPS` is above 5.
    #[error("QUACK_TRUSTED_PROXY_HOPS must be 0 to 5")]
    ProxyHops,
    /// A Discord OAuth URL is not an absolute http(s) URL.
    #[error("invalid Discord OAuth URL: {0}")]
    DiscordUrl(String),
    /// `QUACK_DASHBOARD_DIR` has no `index.html`.
    #[error("QUACK_DASHBOARD_DIR has no index.html: {0}")]
    DashboardDir(String),
}

/// The dashboard's public origin (`QUACK_PUBLIC_URL`): links, the OAuth
/// redirect, the CSRF origin, and the cookie names all come from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl {
    url: Url,
    origin: String,
}

impl PublicUrl {
    /// Parses and checks the public URL: `https`, or `http` only for
    /// `localhost`, `127.0.0.1`, or `[::1]`; no credentials, path, query,
    /// or fragment (a lone trailing slash is accepted).
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let bad = || ConfigError::PublicUrl(text.to_owned());
        let url = Url::parse(text).map_err(|_| bad())?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
        let bare = url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && url.host_str().is_some();
        if !scheme_ok || !bare {
            return Err(bad());
        }
        let origin = url.origin().ascii_serialization();
        Ok(Self { url, origin })
    }

    /// `scheme://host[:port]`, without a trailing slash.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether the dashboard is served over TLS (secure cookies, HSTS).
    pub fn is_https(&self) -> bool {
        self.url.scheme() == "https"
    }

    /// The URL as parsed (path `/`), for resolving relative redirects.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The OAuth redirect URI, `<origin>/api/auth/callback`.
    pub fn callback(&self) -> String {
        format!("{}/api/auth/callback", self.origin)
    }
}

/// The 32-byte session secret. It encrypts the Discord tokens stored in
/// sessions; its fingerprint ties each session to the secret that sealed
/// it. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionSecret([u8; 32]);

impl SessionSecret {
    /// Wraps 32 raw bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses `QUACK_SESSION_SECRET` (standard or URL-safe base64, padded
    /// or not).
    pub fn from_base64(text: &str) -> Result<Self, ConfigError> {
        use base64::Engine as _;
        let text = text.trim();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(text)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(text))
            .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(text))
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text))
            .map_err(|_| ConfigError::SessionSecret)?;
        let bytes: [u8; 32] = decoded.try_into().map_err(|_| ConfigError::SessionSecret)?;
        Ok(Self(bytes))
    }

    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The first 8 bytes of SHA-256 of the secret (`key_fingerprint`).
    pub fn fingerprint(&self) -> [u8; 8] {
        let digest = crate::auth::crypto::sha256(&self.0);
        let mut out = [0_u8; 8];
        out.copy_from_slice(&digest[..8]);
        out
    }
}

impl fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionSecret([redacted])")
    }
}

/// Discord's OAuth application and endpoints. The URLs default to
/// Discord's and are configurable only so tests can fake Discord.
#[derive(Clone, PartialEq, Eq)]
pub struct DiscordOauthConfig {
    /// The application ID (`QUACK_DISCORD_APPLICATION_ID`), the OAuth
    /// client ID.
    pub client_id: String,
    /// The client secret (`QUACK_DISCORD_CLIENT_SECRET`).
    pub client_secret: String,
    /// The authorize page, `https://discord.com/oauth2/authorize`.
    pub authorize_url: String,
    /// The REST base, `https://discord.com/api/v10` (token exchange at
    /// `/oauth2/token`, the account at `/users/@me`).
    pub api_base: String,
}

impl DiscordOauthConfig {
    /// Discord's real endpoints for this application.
    pub fn discord(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            authorize_url: "https://discord.com/oauth2/authorize".to_owned(),
            api_base: "https://discord.com/api/v10".to_owned(),
        }
    }
}

impl fmt::Debug for DiscordOauthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscordOauthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("authorize_url", &self.authorize_url)
            .field("api_base", &self.api_base)
            .finish()
    }
}

/// Everything the API reads from configuration.
#[derive(Clone, Debug)]
pub struct ApiConfig {
    /// `QUACK_PUBLIC_URL`.
    pub public_url: PublicUrl,
    /// `QUACK_SESSION_SECRET`.
    pub session_secret: SessionSecret,
    /// The Discord OAuth application.
    pub discord: DiscordOauthConfig,
    /// `QUACK_TRUSTED_PROXY_HOPS` (0–5): proxies in front of Quack, for
    /// the client IP of `sign_in` rate limits (§14.7).
    pub trusted_proxy_hops: u8,
    /// `QUACK_DASHBOARD_DIR`; `None` serves no dashboard files (tests,
    /// API-only runs).
    pub dashboard_dir: Option<PathBuf>,
}

impl ApiConfig {
    /// Checks what [`PublicUrl`] and [`SessionSecret`] can't: the proxy
    /// hop range, the OAuth URLs, and that the dashboard directory has an
    /// `index.html` (§14.10: `serve` refuses to start otherwise).
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.trusted_proxy_hops > 5 {
            return Err(ConfigError::ProxyHops);
        }
        for url in [&self.discord.authorize_url, &self.discord.api_base] {
            let parsed = Url::parse(url).map_err(|_| ConfigError::DiscordUrl(url.clone()))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(ConfigError::DiscordUrl(url.clone()));
            }
        }
        if let Some(dir) = &self.dashboard_dir
            && !dir.join("index.html").is_file()
        {
            return Err(ConfigError::DashboardDir(dir.display().to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_url_rules() {
        assert!(PublicUrl::parse("https://quack.example").is_ok());
        assert!(PublicUrl::parse("https://quack.example/").is_ok());
        assert!(PublicUrl::parse("http://localhost:5173").is_ok());
        assert!(PublicUrl::parse("http://127.0.0.1:8080").is_ok());
        assert!(PublicUrl::parse("http://quack.example").is_err());
        assert!(PublicUrl::parse("https://quack.example/app").is_err());
        assert!(PublicUrl::parse("https://u:p@quack.example").is_err());
        assert!(PublicUrl::parse("https://quack.example/?a=1").is_err());
        assert_eq!(
            PublicUrl::parse("https://quack.example/").map(|u| u.origin().to_owned()),
            Ok("https://quack.example".to_owned())
        );
    }

    #[test]
    fn session_secret_is_32_bytes_and_redacted() {
        let ok = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
        let secret = SessionSecret::from_base64(ok);
        assert!(secret.is_ok());
        assert_eq!(
            format!("{:?}", secret.ok()),
            "Some(SessionSecret([redacted]))"
        );
        assert_eq!(
            SessionSecret::from_base64("c2hvcnQ="),
            Err(ConfigError::SessionSecret)
        );
    }
}
