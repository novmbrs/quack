//! Downloading attachment files for evidence copies (§7.3): HTTPS from
//! Discord's CDN hosts only, at most 5 redirects within those hosts, one
//! 30-second deadline, and a byte limit checked while reading.

use std::time::Instant;

use quack_telemetry::labels::{HttpMethod, UrlTemplate};
use quack_telemetry::{metrics, spans};
use reqwest::Url;
use tracing::Instrument as _;

use super::outcome::classify_failure;
use super::transport::error_outcome;
use super::{DiscordClient, NotSentCause, Outcome, Refusal, UnknownCause};

/// Where attachments may come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadPolicy {
    /// Allowed host names (exact match).
    pub hosts: &'static [&'static str],
    /// Whether only `https` URLs are allowed.
    pub https_only: bool,
    /// Most redirects followed (each must stay within `hosts`).
    pub max_redirects: usize,
}

impl DownloadPolicy {
    /// Discord's CDN over HTTPS, at most 5 redirects (§7.3).
    pub const DISCORD_CDN: Self = Self {
        hosts: &["cdn.discordapp.com", "media.discordapp.net"],
        https_only: true,
        max_redirects: 5,
    };

    /// Whether a URL may be fetched.
    pub fn allows(&self, url: &Url) -> bool {
        let scheme_ok = url.scheme() == "https" || (!self.https_only && url.scheme() == "http");
        scheme_ok
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some_and(|h| self.hosts.contains(&h))
    }
}

const TEMPLATE: UrlTemplate = UrlTemplate::new("/attachments/{channel_id}/{attachment_id}/{filename}");

/// The download client: redirects are followed only within the policy.
pub(super) fn client(policy: DownloadPolicy) -> reqwest::Result<reqwest::Client> {
    let redirects = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > policy.max_redirects {
            attempt.error("too many redirects")
        } else if policy.allows(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirect left the allowed hosts")
        }
    });
    reqwest::Client::builder()
        .redirect(redirects)
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
}

impl DiscordClient {
    /// Downloads a file, stopping at `max_bytes`. A disallowed URL or
    /// redirect is `NotSent(Invalid)`; a file over the limit is
    /// `Refused(TooLarge)` with status 413 (Quack stopped reading, Discord
    /// did not say so).
    pub async fn download_attachment(&self, url: &str, max_bytes: u64) -> Outcome<Vec<u8>> {
        let span = spans::discord(HttpMethod::Get, TEMPLATE);
        let started = Instant::now();
        let (status, outcome) = self
            .download_in(url, max_bytes)
            .instrument(span.clone())
            .await;
        if let Some(status) = status {
            spans::record_status(&span, status);
        }
        spans::record(&span, outcome.class());
        metrics()
            .discord
            .request(HttpMethod::Get, TEMPLATE, status, outcome.class(), started.elapsed());
        outcome
    }

    async fn download_in(&self, url: &str, max_bytes: u64) -> (Option<u16>, Outcome<Vec<u8>>) {
        let inner = &self.inner;
        if inner.transport.closed.load(std::sync::atomic::Ordering::Acquire) {
            return (None, Outcome::NotSent(NotSentCause::ShuttingDown));
        }
        let Some(url) = Url::parse(url).ok().filter(|u| inner.downloads.allows(u)) else {
            return (None, Outcome::NotSent(NotSentCause::Invalid));
        };
        let deadline = tokio::time::Instant::now() + inner.file_timeout;
        let response = match tokio::time::timeout_at(deadline, inner.downloader.get(url).send()).await
        {
            Err(_) => return (None, Outcome::Unknown(UnknownCause::Timeout)),
            Ok(Err(error)) if error.is_redirect() => {
                return (None, Outcome::NotSent(NotSentCause::Invalid));
            }
            Ok(Err(error)) => return (None, error_outcome(&error)),
            Ok(Ok(response)) => response,
        };
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return (Some(status), classify_failure(status, b"", None));
        }
        let too_large = Outcome::Refused(Refusal::new(413, None));
        if response.content_length().is_some_and(|len| len > max_bytes) {
            return (Some(status), too_large);
        }
        let mut response = response;
        let mut bytes = Vec::new();
        loop {
            match tokio::time::timeout_at(deadline, response.chunk()).await {
                Err(_) => return (Some(status), Outcome::Unknown(UnknownCause::Timeout)),
                Ok(Err(_)) => return (Some(status), Outcome::Unknown(UnknownCause::Connection)),
                Ok(Ok(None)) => return (Some(status), Outcome::Done(bytes)),
                Ok(Ok(Some(chunk))) => {
                    if (bytes.len() + chunk.len()) as u64 > max_bytes {
                        return (Some(status), too_large);
                    }
                    bytes.extend_from_slice(&chunk);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_discord_cdn_over_https() {
        let p = DownloadPolicy::DISCORD_CDN;
        let ok = |u: &str| p.allows(&Url::parse(u).unwrap());
        assert!(ok("https://cdn.discordapp.com/attachments/1/2/a.png?ex=1"));
        assert!(ok("https://media.discordapp.net/attachments/1/2/a.png"));
        assert!(!ok("http://cdn.discordapp.com/attachments/1/2/a.png"));
        assert!(!ok("https://cdn.discordapp.com.evil.example/a.png"));
        assert!(!ok("https://user:pw@cdn.discordapp.com/a.png"));
        assert!(!ok("https://example.com/a.png"));
        assert!(!ok("file:///etc/passwd"));
    }
}
