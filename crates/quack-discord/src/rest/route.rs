//! Every Discord endpoint Quack calls: method, route template (for spans and
//! metrics), concrete path, and the rate-limit path.
//!
//! Templates are static strings, so metric attributes never carry ids.
//! Interaction tokens appear only in the request URL: the rate limiter sees
//! a short hash of the token instead, because Twilight's limiter logs paths.

use std::fmt::Write as _;

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId};
use quack_telemetry::labels::{HttpMethod, UrlTemplate};

/// An HTTP method Discord's API uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub(crate) const fn telemetry(self) -> HttpMethod {
        match self {
            Self::Get => HttpMethod::Get,
            Self::Post => HttpMethod::Post,
            Self::Put => HttpMethod::Put,
            Self::Patch => HttpMethod::Patch,
            Self::Delete => HttpMethod::Delete,
        }
    }

    pub(crate) const fn limiter(self) -> twilight_http_ratelimiting::Method {
        use twilight_http_ratelimiting::Method as M;
        match self {
            Self::Get => M::Get,
            Self::Post => M::Post,
            Self::Put => M::Put,
            Self::Patch => M::Patch,
            Self::Delete => M::Delete,
        }
    }

    pub(crate) const fn reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

/// One concrete request target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) method: Method,
    /// `GET /guilds/{guild_id}/members/{user_id}`'s path part.
    pub(crate) template: &'static str,
    /// The concrete path, starting with `/`, without the query.
    pub(crate) path: String,
    /// The query string without `?`, when any.
    pub(crate) query: Option<String>,
    /// The path the rate limiter buckets by (no leading slash, no token).
    pub(crate) limit_path: String,
    /// Whether the bot token is sent (interaction webhooks don't need it).
    pub(crate) auth: bool,
}

/// The initial interaction response's route.
const INTERACTION_CALLBACK: &str = "/interactions/{interaction_id}/{interaction_token}/callback";

/// A short, stable stand-in for an interaction token (FNV-1a, hex).
fn token_tag(token: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in token.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("t{hash:016x}")
}

/// The path Discord buckets by: its major resource ids (a channel, a
/// server, a webhook) kept, every other id (a message, a member, a role)
/// replaced, so two messages of one channel share their bucket.
fn limit_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.trim_start_matches('/').split('/') {
        let major = matches!(out.last(), Some(&("channels" | "guilds" | "webhooks")));
        let id = !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
        out.push(if id && !major { "{id}" } else { segment });
    }
    out.join("/")
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
pub(crate) fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

impl Route {
    fn bot(method: Method, template: &'static str, path: String) -> Self {
        Self {
            method,
            template,
            limit_path: limit_path(&path),
            path,
            query: None,
            auth: true,
        }
    }

    fn with_query(mut self, query: String) -> Self {
        self.query = Some(query);
        self
    }

    /// The span and metric template, e.g. `/guilds/{guild_id}/bans/{user_id}`.
    pub(crate) const fn url_template(&self) -> UrlTemplate {
        UrlTemplate::new(self.template)
    }

    // Accounts and the gateway.

    pub(crate) fn current_user() -> Self {
        Self::bot(Method::Get, "/users/@me", "/users/@me".into())
    }
    pub(crate) fn user(user: AccountId) -> Self {
        Self::bot(Method::Get, "/users/{user_id}", format!("/users/{user}"))
    }
    pub(crate) fn gateway_bot() -> Self {
        Self::bot(Method::Get, "/gateway/bot", "/gateway/bot".into())
    }

    // Servers, members, bans.

    pub(crate) fn guild(guild: GuildId) -> Self {
        Self::bot(
            Method::Get,
            "/guilds/{guild_id}",
            format!("/guilds/{guild}"),
        )
    }
    pub(crate) fn guild_channels(guild: GuildId) -> Self {
        Self::bot(
            Method::Get,
            "/guilds/{guild_id}/channels",
            format!("/guilds/{guild}/channels"),
        )
    }
    pub(crate) fn member(method: Method, guild: GuildId, member: MemberId) -> Self {
        Self::bot(
            method,
            "/guilds/{guild_id}/members/{user_id}",
            format!("/guilds/{guild}/members/{member}"),
        )
    }
    pub(crate) fn search_members(guild: GuildId, query: &str, limit: u8) -> Self {
        Self::bot(
            Method::Get,
            "/guilds/{guild_id}/members/search",
            format!("/guilds/{guild}/members/search"),
        )
        .with_query(format!("query={}&limit={limit}", encode_component(query)))
    }
    pub(crate) fn ban(method: Method, guild: GuildId, member: MemberId) -> Self {
        Self::bot(
            method,
            "/guilds/{guild_id}/bans/{user_id}",
            format!("/guilds/{guild}/bans/{member}"),
        )
    }
    pub(crate) fn active_threads(guild: GuildId) -> Self {
        Self::bot(
            Method::Get,
            "/guilds/{guild_id}/threads/active",
            format!("/guilds/{guild}/threads/active"),
        )
    }
    pub(crate) fn create_channel(guild: GuildId) -> Self {
        Self::bot(
            Method::Post,
            "/guilds/{guild_id}/channels",
            format!("/guilds/{guild}/channels"),
        )
    }

    // Channels and threads.

    pub(crate) fn channel(method: Method, channel: ChannelId) -> Self {
        Self::bot(
            method,
            "/channels/{channel_id}",
            format!("/channels/{channel}"),
        )
    }
    pub(crate) fn dm_channel() -> Self {
        Self::bot(
            Method::Post,
            "/users/@me/channels",
            "/users/@me/channels".into(),
        )
    }
    pub(crate) fn create_thread(parent: ChannelId) -> Self {
        Self::bot(
            Method::Post,
            "/channels/{channel_id}/threads",
            format!("/channels/{parent}/threads"),
        )
    }
    pub(crate) fn archived_private_threads(parent: ChannelId, before: Option<&str>) -> Self {
        let route = Self::bot(
            Method::Get,
            "/channels/{channel_id}/threads/archived/private",
            format!("/channels/{parent}/threads/archived/private"),
        );
        match before {
            Some(before) => {
                route.with_query(format!("limit=100&before={}", encode_component(before)))
            }
            None => route.with_query("limit=100".into()),
        }
    }
    pub(crate) fn thread_member(method: Method, thread: ChannelId, user: AccountId) -> Self {
        Self::bot(
            method,
            "/channels/{channel_id}/thread-members/{user_id}",
            format!("/channels/{thread}/thread-members/{user}"),
        )
    }
    pub(crate) fn thread_members(thread: ChannelId) -> Self {
        Self::bot(
            Method::Get,
            "/channels/{channel_id}/thread-members",
            format!("/channels/{thread}/thread-members"),
        )
    }

    // Messages.

    pub(crate) fn messages(method: Method, channel: ChannelId) -> Self {
        Self::bot(
            method,
            "/channels/{channel_id}/messages",
            format!("/channels/{channel}/messages"),
        )
    }
    pub(crate) fn history(channel: ChannelId, before: Option<MessageId>, limit: u8) -> Self {
        let route = Self::messages(Method::Get, channel);
        match before {
            Some(before) => route.with_query(format!("limit={limit}&before={before}")),
            None => route.with_query(format!("limit={limit}")),
        }
    }
    pub(crate) fn message(method: Method, channel: ChannelId, message: MessageId) -> Self {
        Self::bot(
            method,
            "/channels/{channel_id}/messages/{message_id}",
            format!("/channels/{channel}/messages/{message}"),
        )
    }

    // Interactions (no bot token; the token is in the path).

    /// Whether this is an interaction's initial response, which never
    /// waits for a rate-limit permit (§17.3).
    pub(crate) fn is_callback(&self) -> bool {
        self.template == INTERACTION_CALLBACK
    }

    pub(crate) fn interaction_callback(id: u64, token: &str) -> Self {
        Self {
            method: Method::Post,
            template: INTERACTION_CALLBACK,
            path: format!("/interactions/{id}/{token}/callback"),
            query: None,
            limit_path: format!("interactions/{id}/{}/callback", token_tag(token)),
            auth: false,
        }
    }
    pub(crate) fn webhook_message(method: Method, app: u64, token: &str, message: &str) -> Self {
        Self {
            method,
            template: "/webhooks/{application_id}/{interaction_token}/messages/{message_id}",
            path: format!("/webhooks/{app}/{token}/messages/{message}"),
            query: None,
            limit_path: format!("webhooks/{app}/{}/messages/{message}", token_tag(token)),
            auth: false,
        }
    }
    pub(crate) fn followup(app: u64, token: &str) -> Self {
        Self {
            method: Method::Post,
            template: "/webhooks/{application_id}/{interaction_token}",
            path: format!("/webhooks/{app}/{token}"),
            query: Some("wait=true".into()),
            limit_path: format!("webhooks/{app}/{}", token_tag(token)),
            auth: false,
        }
    }

    // Application commands.

    pub(crate) fn commands(method: Method, app: u64, guild: Option<GuildId>) -> Self {
        match guild {
            Some(guild) => Self::bot(
                method,
                "/applications/{application_id}/guilds/{guild_id}/commands",
                format!("/applications/{app}/guilds/{guild}/commands"),
            ),
            None => Self::bot(
                method,
                "/applications/{application_id}/commands",
                format!("/applications/{app}/commands"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_never_reach_the_limiter_path() {
        let token = "aW50ZXJhY3Rpb246MTIzNDU2Nzg5OmFiY2RlZg";
        for route in [
            Route::interaction_callback(5, token),
            Route::webhook_message(Method::Patch, 7, token, "@original"),
            Route::followup(7, token),
        ] {
            assert!(route.path.contains(token));
            assert!(!route.limit_path.contains(token));
            assert!(!route.template.contains(token));
            assert!(!route.auth);
            assert!(!route.limit_path.starts_with('/'));
        }
        assert_eq!(token_tag(token), token_tag(token));
        assert_ne!(token_tag(token), token_tag("other"));
    }

    #[test]
    fn minor_ids_share_a_bucket_and_major_ones_dont() {
        assert_eq!(
            limit_path("/channels/5/messages/10"),
            limit_path("/channels/5/messages/11")
        );
        assert_eq!(
            limit_path("/channels/5/messages/10"),
            "channels/5/messages/{id}"
        );
        assert_ne!(
            limit_path("/channels/5/messages/10"),
            limit_path("/channels/6/messages/10")
        );
        assert_eq!(
            limit_path("/guilds/1/members/7/roles/9"),
            "guilds/1/members/{id}/roles/{id}"
        );
        assert_eq!(limit_path("/users/@me"), "users/@me");
    }

    #[test]
    fn queries_are_encoded_and_kept_out_of_the_limiter_path() {
        let guild = GuildId::new(1).unwrap();
        let route = Route::search_members(guild, "a b&c=ü", 5);
        assert_eq!(
            route.query.as_deref(),
            Some("query=a%20b%26c%3D%C3%BC&limit=5")
        );
        assert_eq!(route.limit_path, "guilds/1/members/search");
        assert_eq!(
            route.url_template().as_str(),
            "/guilds/{guild_id}/members/search"
        );
    }
}
