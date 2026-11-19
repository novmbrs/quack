//! Channels and threads: reads for facts and reconciliation, setup's
//! channel creation (§13.1.4), and ticket threads (§13.2).

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId};
use quack_core::permissions::{Overwrite, OverwriteTarget};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::route::{Method, Route};
use super::transport::Request;
use super::{DiscordClient, Outcome, RefusalKind, UnknownCause, converted, with_json};
use crate::convert;
use crate::model::{Channel, ChannelSpec};

#[derive(Serialize)]
struct OverwriteBody {
    id: String,
    #[serde(rename = "type")]
    kind: u8,
    allow: String,
    deny: String,
}

impl From<&Overwrite> for OverwriteBody {
    fn from(o: &Overwrite) -> Self {
        let (id, kind) = match o.target {
            OverwriteTarget::Role(role) => (role.to_string(), 0),
            OverwriteTarget::Member(member) => (member.to_string(), 1),
        };
        Self {
            id,
            kind,
            allow: o.allow.0.to_string(),
            deny: o.deny.0.to_string(),
        }
    }
}

#[derive(Serialize)]
struct CreateChannel<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    topic: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    permission_overwrites: Vec<OverwriteBody>,
}

#[derive(Serialize)]
struct CreateThread<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    kind: u8,
    invitable: bool,
    auto_archive_duration: u16,
}

#[derive(Serialize)]
struct ThreadStateBody {
    archived: bool,
    locked: bool,
}

#[derive(Deserialize)]
struct ThreadList {
    threads: Vec<twilight_model::channel::Channel>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Deserialize)]
struct ThreadMemberRow {
    user_id: Option<twilight_model::id::Id<twilight_model::id::marker::UserMarker>>,
}

/// One page of archived threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadPage {
    /// The threads on this page.
    pub threads: Vec<Channel>,
    /// Whether older threads remain (ask again with the oldest's archive
    /// time as `before`).
    pub has_more: bool,
}

/// Most archived-thread pages [`DiscordClient::find_threads`] reads (100
/// threads each), so one lookup stays bounded.
pub const FIND_THREADS_MAX_PAGES: usize = 10;

/// Discord's channel type of a private thread.
const PRIVATE_THREAD: u8 = 12;

fn channels(list: &[twilight_model::channel::Channel]) -> Vec<Channel> {
    list.iter().filter_map(convert::channel).collect()
}

impl DiscordClient {
    /// `GET /channels/{id}`: a channel or thread. `Refused(UnknownChannel)`
    /// means it is gone (§13.1.5); 403 is not a deletion.
    pub async fn channel(&self, channel: ChannelId) -> Outcome<Channel> {
        self.json::<twilight_model::channel::Channel>(Request::new(Route::channel(
            Method::Get,
            channel,
        )))
        .await
        .and_then(|c| converted(convert::channel(&c)))
    }

    /// `GET /guilds/{id}/channels`: every channel of the server (not
    /// threads).
    pub async fn guild_channels(&self, guild: GuildId) -> Outcome<Vec<Channel>> {
        self.json::<Vec<twilight_model::channel::Channel>>(Request::new(Route::guild_channels(
            guild,
        )))
        .await
        .map(|list| channels(&list))
    }

    /// `GET /guilds/{id}/threads/active`: every active thread Quack can see.
    pub async fn active_threads(&self, guild: GuildId) -> Outcome<Vec<Channel>> {
        self.json::<ThreadList>(Request::new(Route::active_threads(guild)))
            .await
            .map(|list| channels(&list.threads))
    }

    /// `GET /channels/{id}/threads/archived/private`: archived private
    /// threads, newest first, 100 per page; `before` is an ISO 8601 archive
    /// time from the previous page.
    pub async fn archived_private_threads(
        &self,
        parent: ChannelId,
        before: Option<&str>,
    ) -> Outcome<ThreadPage> {
        self.json::<ThreadList>(Request::new(Route::archived_private_threads(
            parent, before,
        )))
        .await
        .map(|list| ThreadPage {
            threads: channels(&list.threads),
            has_more: list.has_more,
        })
    }

    /// Quack's own ticket threads under `parent` named exactly `name`,
    /// oldest first: private threads (type 12) of `guild` that `creator`
    /// (Quack's account) created, among the server's active threads and the
    /// archived private threads under `parent`. Ticket threads are found
    /// this way before one is created, because threads have no nonce
    /// (§10.4, §13.2.3); nothing anyone else made is ever returned, so a
    /// caller may adopt or delete what it gets.
    ///
    /// Archived pages come newest-archived first; the lookup is complete
    /// once Discord has no more pages or a page reaches threads archived
    /// before `since` (a thread made after `since` can't be older). Reaching
    /// [`FIND_THREADS_MAX_PAGES`] first is `Unknown(Timeout)` and a page
    /// without its cursor `Unknown(Malformed)`: an incomplete lookup never
    /// authorizes a create or a delete.
    pub async fn find_threads(
        &self,
        guild: GuildId,
        parent: ChannelId,
        name: &str,
        creator: AccountId,
        since: OffsetDateTime,
    ) -> Outcome<Vec<ChannelId>> {
        let ours = |c: &Channel| {
            c.guild_id == Some(guild)
                && c.parent_id == Some(parent)
                && c.kind == PRIVATE_THREAD
                && c.name == name
                && c.owner_id == Some(creator)
        };
        let mut found: Vec<ChannelId> = match self.active_threads(guild).await.into_result() {
            Ok(active) => active.iter().filter(|c| ours(c)).map(|c| c.id).collect(),
            Err(failure) => return failure,
        };
        let mut before: Option<String> = None;
        let mut complete = false;
        for _ in 0..FIND_THREADS_MAX_PAGES {
            let page = match self
                .archived_private_threads(parent, before.as_deref())
                .await
                .into_result()
            {
                Ok(page) => page,
                Err(failure) => return failure,
            };
            found.extend(page.threads.iter().filter(|c| ours(c)).map(|c| c.id));
            let oldest = page.threads.last().and_then(|c| c.thread?.archived_at);
            if !page.has_more || oldest.is_some_and(|at| at < since) {
                complete = true;
                break;
            }
            let Some(cursor) = oldest.and_then(|at| at.format(&Rfc3339).ok()) else {
                return Outcome::Unknown(UnknownCause::Malformed);
            };
            before = Some(cursor);
        }
        if !complete {
            return Outcome::Unknown(UnknownCause::Timeout);
        }
        // Snowflakes grow with creation time.
        found.sort_unstable();
        found.dedup();
        Outcome::Done(found)
    }

    /// Creates a text channel with the given overwrites (§13.1.4). Channels
    /// have no nonce: look for one Quack just made before calling again.
    pub async fn create_channel(&self, guild: GuildId, spec: &ChannelSpec) -> Outcome<ChannelId> {
        let body = CreateChannel {
            name: &spec.name,
            kind: 0,
            topic: spec.topic.as_deref(),
            parent_id: spec.parent_id.map(|p| p.to_string()),
            permission_overwrites: spec.overwrites.iter().map(OverwriteBody::from).collect(),
        };
        self.json::<twilight_model::channel::Channel>(with_json(
            Route::create_channel(guild),
            &body,
        ))
        .await
        .and_then(|c| converted(convert::id(c.id)))
    }

    /// Creates a private thread under `parent` that members can't invite to
    /// and that archives after a day (§13.2.3).
    pub async fn create_private_thread(&self, parent: ChannelId, name: &str) -> Outcome<ChannelId> {
        let body = CreateThread {
            name,
            kind: 12,
            invitable: false,
            auto_archive_duration: 1440,
        };
        self.json::<twilight_model::channel::Channel>(with_json(
            Route::create_thread(parent),
            &body,
        ))
        .await
        .and_then(|c| converted(convert::id(c.id)))
    }

    /// Whether an account is a member of a thread: `Done(false)` for
    /// Discord's 404 Unknown Member (§7.2).
    pub async fn is_thread_member(&self, thread: ChannelId, account: AccountId) -> Outcome<bool> {
        match self
            .call(Request::new(Route::thread_member(
                Method::Get,
                thread,
                account,
            )))
            .await
        {
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownMember => Outcome::Done(false),
            other => other.map(|_| true),
        }
    }

    /// Every member of a thread (§13.2.4 "Fix access").
    pub async fn thread_members(&self, thread: ChannelId) -> Outcome<Vec<MemberId>> {
        self.json::<Vec<ThreadMemberRow>>(Request::new(Route::thread_members(thread)))
            .await
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r.user_id.and_then(convert::id::<MemberId, _>))
                    .collect()
            })
    }

    /// Adds a member to a thread (idempotent).
    pub async fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> Outcome<()> {
        self.unit(Request::new(Route::thread_member(
            Method::Put,
            thread,
            member.account(),
        )))
        .await
    }

    /// Removes a member from a thread.
    pub async fn remove_thread_member(&self, thread: ChannelId, member: MemberId) -> Outcome<()> {
        self.unit(Request::new(Route::thread_member(
            Method::Delete,
            thread,
            member.account(),
        )))
        .await
    }

    /// Archives or unarchives, locks or unlocks a thread.
    pub async fn set_thread_state(
        &self,
        thread: ChannelId,
        archived: bool,
        locked: bool,
    ) -> Outcome<()> {
        self.object(with_json(
            Route::channel(Method::Patch, thread),
            &ThreadStateBody { archived, locked },
        ))
        .await
    }

    /// Deletes a channel or thread.
    pub async fn delete_channel(&self, channel: ChannelId) -> Outcome<()> {
        self.object(Request::new(Route::channel(Method::Delete, channel)))
            .await
    }
}
