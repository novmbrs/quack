//! Reading and checking one configuration value at a time, collecting
//! every problem.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::{ConfigErrors, KEYS, Secret};

/// Collected problems.
#[derive(Debug, Default)]
pub(super) struct Problems(Vec<String>);

fn env_name(key: &str) -> String {
    format!("QUACK_{}", key.to_ascii_uppercase())
}

fn get<'a>(v: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    v.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// A text value with a default.
pub(super) fn text(v: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    get(v, key).unwrap_or(default).to_owned()
}

impl Problems {
    fn push(&mut self, key: &str, what: &str) {
        self.0.push(format!("{} {what}", env_name(key)));
    }

    /// The value, or a problem when it is required and missing.
    pub(super) fn required(
        &mut self,
        v: &BTreeMap<String, String>,
        key: &str,
        needed: bool,
    ) -> Option<String> {
        let found = get(v, key).map(str::to_owned);
        if needed && found.is_none() {
            self.push(key, "is required.");
        }
        found
    }

    /// A whole number in `range`, with a default.
    pub(super) fn number<T>(
        &mut self,
        v: &BTreeMap<String, String>,
        key: &str,
        range: RangeInclusive<T>,
        default: T,
    ) -> T
    where
        T: FromStr + PartialOrd + Copy + std::fmt::Display,
    {
        let Some(text) = get(v, key) else {
            return default;
        };
        match text.parse::<T>() {
            Ok(n) if range.contains(&n) => n,
            _ => {
                self.push(
                    key,
                    &format!(
                        "must be a whole number from {} to {}.",
                        range.start(),
                        range.end()
                    ),
                );
                default
            }
        }
    }

    /// A Discord snowflake (`1..=i64::MAX`).
    pub(super) fn snowflake(
        &mut self,
        v: &BTreeMap<String, String>,
        key: &str,
        needed: bool,
    ) -> Option<u64> {
        let text = self.required(v, key, needed)?;
        match text.parse::<u64>() {
            Ok(n) if n > 0 && i64::try_from(n).is_ok() => Some(n),
            _ => {
                self.push(
                    key,
                    "must be a Discord ID (a number like 123456789012345678).",
                );
                None
            }
        }
    }

    /// A socket address, with a default.
    pub(super) fn socket(
        &mut self,
        v: &BTreeMap<String, String>,
        key: &str,
        default: &str,
    ) -> SocketAddr {
        let text = get(v, key).unwrap_or(default);
        text.parse().unwrap_or_else(|_| {
            self.push(key, "must be an address and port, like 0.0.0.0:8080.");
            SocketAddr::from(([0, 0, 0, 0], 8080))
        })
    }

    /// An absolute http(s) URL, without a trailing slash.
    pub(super) fn url(&mut self, v: &BTreeMap<String, String>, key: &str) -> Option<String> {
        let text = get(v, key)?;
        match reqwest::Url::parse(text) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {
                Some(text.trim_end_matches('/').to_owned())
            }
            _ => {
                self.push(key, "must be an absolute http or https URL.");
                None
            }
        }
    }

    /// `QUACK_PUBLIC_URL`: `https`, or `http` only for localhost; no
    /// credentials, path, query, or fragment.
    pub(super) fn public_url(
        &mut self,
        v: &BTreeMap<String, String>,
        needed: bool,
    ) -> Option<String> {
        let text = self.required(v, "public_url", needed)?;
        let ok = reqwest::Url::parse(&text).ok().filter(|url| {
            let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
            (url.scheme() == "https" || (url.scheme() == "http" && loopback))
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        });
        if ok.is_none() {
            self.push(
                "public_url",
                "must be an https origin like https://quack.example.com (http only for localhost), with no path, query, or login.",
            );
            return None;
        }
        Some(text.trim_end_matches('/').to_owned())
    }

    /// `QUACK_SESSION_SECRET`: base64 of exactly 32 bytes.
    pub(super) fn session_secret(
        &mut self,
        v: &BTreeMap<String, String>,
        needed: bool,
    ) -> Option<Secret> {
        let text = self.required(v, "session_secret", needed)?;
        if base64_len(&text) == Some(32) {
            Some(Secret::new(text))
        } else {
            self.push(
                "session_secret",
                "must be base64 of exactly 32 bytes (for example the output of `openssl rand -base64 32`).",
            );
            None
        }
    }

    /// `QUACK_DASHBOARD_DIR`: must hold `index.html` for `serve`.
    pub(super) fn dashboard_dir(&mut self, v: &BTreeMap<String, String>, check: bool) -> PathBuf {
        let dir = PathBuf::from(get(v, "dashboard_dir").unwrap_or("dashboard"));
        if check && !dir.join("index.html").is_file() {
            self.push(
                "dashboard_dir",
                &format!(
                    "must be a directory with the built dashboard (no index.html in {}).",
                    dir.display()
                ),
            );
        }
        dir
    }

    /// `QUACK_LOG_LEVEL`: a `tracing` filter directive.
    pub(super) fn log_level(&mut self, v: &BTreeMap<String, String>) -> String {
        let text = get(v, "log_level").unwrap_or("info");
        if tracing_subscriber::EnvFilter::try_new(text).is_err() {
            self.push(
                "log_level",
                "must be a log filter like info or info,quack_db=debug.",
            );
            return "info".to_owned();
        }
        text.to_owned()
    }

    /// `QUACK_OPS_GUILD_ID`, `QUACK_OPS_USER_IDS`, and
    /// `QUACK_OPS_TRACE_URL` (§21): all or nothing.
    pub(super) fn ops(&mut self, v: &BTreeMap<String, String>) -> Option<super::OpsSettings> {
        use quack_core::ids::{AccountId, GuildId};
        let guild = self.snowflake(v, "ops_guild_id", false);
        let users = get(v, "ops_user_ids");
        let trace = get(v, "ops_trace_url");
        if guild.is_none() {
            if users.is_some() || trace.is_some() {
                self.push(
                    "ops_guild_id",
                    "is required when QUACK_OPS_USER_IDS or QUACK_OPS_TRACE_URL is set.",
                );
            }
            return None;
        }
        let mut operators = Vec::new();
        let mut bad = users.is_none();
        for part in users.unwrap_or_default().split(',') {
            match part.trim().parse::<u64>().ok().and_then(AccountId::new) {
                Some(id) if !operators.contains(&id) => operators.push(id),
                Some(_) => {}
                None => bad = true,
            }
        }
        if bad || operators.is_empty() || operators.len() > 25 {
            self.push(
                "ops_user_ids",
                "must list 1 to 25 Discord account IDs, separated by commas.",
            );
        }
        let trace_url = trace.and_then(|t| {
            let url = t.replace("{trace_id}", "0");
            let ok = t.contains("{trace_id}")
                && reqwest::Url::parse(&url).is_ok_and(|u| {
                    matches!(u.scheme(), "http" | "https") && u.host_str().is_some()
                });
            if ok {
                Some(t.to_owned())
            } else {
                self.push(
                    "ops_trace_url",
                    "must be an absolute http or https URL containing {trace_id}.",
                );
                None
            }
        });
        Some(super::OpsSettings {
            guild: GuildId::new(guild?)?,
            operators,
            trace_url,
        })
    }

    /// The configuration, or every problem.
    pub(super) fn finish<T>(self, value: T) -> Result<T, ConfigErrors> {
        if self.0.is_empty() {
            Ok(value)
        } else {
            Err(ConfigErrors(self.0))
        }
    }
}

/// The decoded length of standard or URL-safe base64 (padding optional),
/// or `None` when it isn't base64.
pub(super) fn base64_len(text: &str) -> Option<usize> {
    let body = text.trim_end_matches('=');
    let padding = text.len() - body.len();
    let valid = body
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
    if !valid
        || padding > 2
        || body.len() % 4 == 1
        || (padding > 0 && !text.len().is_multiple_of(4))
    {
        return None;
    }
    Some(body.len() * 3 / 4)
}

/// Why the config file couldn't be read, without its content: a parse
/// error's own message quotes the offending line, which may hold a secret.
fn unreadable(path: &Path, error: &config::ConfigError) -> String {
    let file = path.display();
    match error {
        config::ConfigError::FileParse { cause, .. } => {
            let at = location(&cause.to_string()).map_or_else(String::new, |(line, column)| {
                format!(" at line {line}, column {column}")
            });
            format!("The config file {file} isn't valid TOML{at}.")
        }
        config::ConfigError::NotFound(_) => format!("The config file {file} doesn't exist."),
        _ => format!("Couldn't read the config file {file}."),
    }
}

/// The line and column from the first line of a TOML parse error
/// (`TOML parse error at line 1, column 17`); nothing else of it is kept.
fn location(message: &str) -> Option<(u32, u32)> {
    let rest = message
        .lines()
        .next()?
        .strip_prefix("TOML parse error at line ")?;
    let (line, column) = rest.split_once(", column ")?;
    Some((line.trim().parse().ok()?, column.trim().parse().ok()?))
}

/// Reads a TOML file with the configuration keys (lowercase, no prefix).
/// Unknown keys are an error.
pub(super) fn file(path: &Path) -> Result<BTreeMap<String, String>, ConfigErrors> {
    let read = config::Config::builder()
        .add_source(
            config::File::from(path)
                .format(config::FileFormat::Toml)
                .required(true),
        )
        .build()
        .and_then(config::Config::try_deserialize::<BTreeMap<String, config::Value>>)
        .map_err(|e| ConfigErrors(vec![unreadable(path, &e)]))?;
    let mut out = BTreeMap::new();
    let mut problems = Vec::new();
    for (key, value) in read {
        if !KEYS.contains(&key.as_str()) {
            problems.push(format!("The config file has an unknown key `{key}`."));
            continue;
        }
        match value.into_string() {
            Ok(text) => {
                out.insert(key, text);
            }
            Err(_) => problems.push(format!("The config file's `{key}` must be a single value.")),
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(ConfigErrors(problems))
    }
}
