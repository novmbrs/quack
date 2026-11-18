//! Configuration (§17.7): `QUACK_*` environment variables, or an optional
//! TOML file with the same names in lowercase without the prefix.
//! Environment variables win over the file; unknown file keys are an
//! error. Every key is validated at once and every problem reported
//! together; secrets never appear in `Debug` output or logs.

mod parse;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use quack_core::ids::GuildId;

pub use quack_core::Secret;

/// Every key Quack reads, without the `QUACK_` prefix, in lowercase.
pub const KEYS: [&str; 17] = [
    "database_url",
    "database_max_connections",
    "discord_token",
    "discord_application_id",
    "discord_client_secret",
    "public_url",
    "listen_addr",
    "session_secret",
    "dashboard_dir",
    "otlp_endpoint",
    "service_name",
    "environment",
    "log_level",
    "worker_concurrency",
    "dev_guild_id",
    "trusted_proxy_hops",
    "import_source_url",
];

/// Which command the configuration is for: each needs different keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// `quack serve`: every serve key.
    Serve,
    /// `quack migrate`: the database.
    Migrate,
    /// `quack commands sync`: the Discord token and application id.
    CommandsSync,
    /// `quack import-mysql`: both databases.
    Import,
}

/// The Discord application Quack runs as.
#[derive(Clone, Debug)]
pub struct DiscordConfig {
    /// The bot token (without `Bot `).
    pub token: Secret,
    /// The application id (also the OAuth client id).
    pub application_id: u64,
}

/// Validated configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// `QUACK_DATABASE_URL` (it may hold a password).
    pub database_url: Secret,
    /// `QUACK_DATABASE_MAX_CONNECTIONS`, 4–200.
    pub database_max_connections: u32,
    /// `QUACK_DISCORD_TOKEN` and `QUACK_DISCORD_APPLICATION_ID`.
    pub discord: Option<DiscordConfig>,
    /// `QUACK_DISCORD_CLIENT_SECRET`.
    pub discord_client_secret: Option<Secret>,
    /// `QUACK_PUBLIC_URL`, without a trailing slash.
    pub public_url: Option<String>,
    /// `QUACK_LISTEN_ADDR`.
    pub listen_addr: SocketAddr,
    /// `QUACK_SESSION_SECRET`, decoded (32 bytes).
    pub session_secret: Option<Secret>,
    /// `QUACK_DASHBOARD_DIR`.
    pub dashboard_dir: PathBuf,
    /// `QUACK_OTLP_ENDPOINT`.
    pub otlp_endpoint: Option<String>,
    /// `QUACK_SERVICE_NAME`.
    pub service_name: String,
    /// `QUACK_ENVIRONMENT`.
    pub environment: String,
    /// `QUACK_LOG_LEVEL`, a `tracing` filter directive.
    pub log_level: String,
    /// `QUACK_WORKER_CONCURRENCY`, 1–64.
    pub worker_concurrency: usize,
    /// `QUACK_DEV_GUILD_ID`.
    pub dev_guild_id: Option<GuildId>,
    /// `QUACK_TRUSTED_PROXY_HOPS`, 0–5.
    pub trusted_proxy_hops: u8,
    /// `QUACK_IMPORT_SOURCE_URL`.
    pub import_source_url: Option<Secret>,
}

impl Config {
    /// The first 8 bytes of SHA-256 of the decoded session secret (the
    /// API's `key_fingerprint`; sessions sealed under another secret are
    /// deleted by housekeeping, §14.4). Zero without a valid secret.
    pub fn session_key_fingerprint(&self) -> [u8; 8] {
        self.session_secret
            .as_ref()
            .and_then(|s| quack_api::SessionSecret::from_base64(s.expose()).ok())
            .map_or([0; 8], |s| s.fingerprint())
    }
}

/// Every problem found, one sentence each.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.join("\n"))]
pub struct ConfigErrors(pub Vec<String>);

/// Reads the values: the file's (if any), then `QUACK_*` variables over
/// them, keyed by lowercase name.
pub fn values(
    file: Option<&Path>,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<BTreeMap<String, String>, ConfigErrors> {
    let mut out = match file {
        Some(path) => parse::file(path)?,
        None => BTreeMap::new(),
    };
    for (key, value) in env {
        if let Some(name) = key.strip_prefix("QUACK_") {
            let name = name.to_ascii_lowercase();
            if KEYS.contains(&name.as_str()) {
                out.insert(name, value);
            }
        }
    }
    Ok(out)
}

/// Loads and validates the configuration for `need` from the process
/// environment and an optional file.
pub fn load(need: Need, file: Option<&Path>) -> Result<Config, ConfigErrors> {
    validate(need, &values(file, std::env::vars())?)
}

/// Validates every key, reporting all problems at once.
pub fn validate(need: Need, v: &BTreeMap<String, String>) -> Result<Config, ConfigErrors> {
    let mut p = parse::Problems::default();
    let serve = need == Need::Serve;
    let discord_needed = matches!(need, Need::Serve | Need::CommandsSync);
    let database_url = p.required(v, "database_url", need != Need::CommandsSync);
    let token = p.required(v, "discord_token", discord_needed);
    let application_id = p.snowflake(v, "discord_application_id", discord_needed);
    let config = Config {
        database_url: Secret::new(database_url.unwrap_or_default()),
        database_max_connections: p.number(v, "database_max_connections", 4..=200, 20),
        discord: token
            .zip(application_id)
            .map(|(token, application_id)| DiscordConfig {
                token: Secret::new(token),
                application_id,
            }),
        discord_client_secret: p
            .required(v, "discord_client_secret", serve)
            .map(Secret::new),
        public_url: p.public_url(v, serve),
        listen_addr: p.socket(v, "listen_addr", "0.0.0.0:8080"),
        session_secret: p.session_secret(v, serve),
        dashboard_dir: p.dashboard_dir(v, serve),
        otlp_endpoint: p.url(v, "otlp_endpoint"),
        service_name: parse::text(v, "service_name", "quack"),
        environment: parse::text(v, "environment", "development"),
        log_level: p.log_level(v),
        worker_concurrency: p.number(v, "worker_concurrency", 1..=64, 16),
        dev_guild_id: p.snowflake(v, "dev_guild_id", false).and_then(GuildId::new),
        trusted_proxy_hops: p.number(v, "trusted_proxy_hops", 0..=5, 0),
        import_source_url: p
            .required(v, "import_source_url", need == Need::Import)
            .map(Secret::new),
    };
    p.finish(config)
}
