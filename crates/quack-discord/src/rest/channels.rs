//! Channels and threads: reads for facts and reconciliation, setup's
//! channel creation (§13.1.4), and ticket threads (§13.2).

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId};
use quack_core::permissions::{Overwrite, OverwriteTarget};
use serde::{Deserialize, Serialize};

use super::route::{Method, Route};
use super::transport::Request;
use super::{DiscordClient, Outcome, RefusalKind, converted, with_json};
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
    pub async fn thread_members(&self, thread: ChannelId) -> Outcome<Vec<AccountId>> {
        self.json::<Vec<ThreadMemberRow>>(Request::new(Route::thread_members(thread)))
            .await
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r.user_id.and_then(convert::id))
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
        self.unit(with_json(
            Route::channel(Method::Patch, thread),
            &ThreadStateBody { archived, locked },
        ))
        .await
    }

    /// Deletes a channel or thread.
    pub async fn delete_channel(&self, channel: ChannelId) -> Outcome<()> {
        self.unit(Request::new(Route::channel(Method::Delete, channel)))
            .await
    }
}
