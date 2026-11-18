//! Accounts, servers, members, and bans: the reads facts come from, and the
//! five moderation actions (§5.1).

use quack_core::ids::{AccountId, GuildId, MemberId};
use quack_core::permissions::{AccountFacts, BanFacts, GuildFacts};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::route::{Method, Route};
use super::transport::Request;
use super::{DiscordClient, NotSentCause, Outcome, RefusalKind, converted, with_json};
use crate::convert;
use crate::model::{GatewayBot, Guild, Member};
use crate::permissions::member_facts;

#[derive(Serialize)]
struct TimeoutBody {
    communication_disabled_until: Option<String>,
}

#[derive(Serialize)]
struct BanBody {
    delete_message_seconds: u32,
}

impl DiscordClient {
    /// `GET /users/@me`: validates the token and names Quack's account.
    pub async fn current_user(&self) -> Outcome<AccountFacts> {
        self.json::<twilight_model::user::User>(Request::new(Route::current_user()))
            .await
            .and_then(|u| converted(convert::user(&u)))
    }

    /// `GET /users/{id}`: an account, whether or not it shares a server.
    pub async fn user(&self, user: AccountId) -> Outcome<AccountFacts> {
        self.json::<twilight_model::user::User>(Request::new(Route::user(user)))
            .await
            .and_then(|u| converted(convert::user(&u)))
    }

    /// `GET /gateway/bot`: how many shards Discord recommends (§11.9).
    pub async fn gateway_bot(&self) -> Outcome<GatewayBot> {
        self.json::<twilight_model::gateway::connection_info::BotConnectionInfo>(Request::new(
            Route::gateway_bot(),
        ))
        .await
        .map(|info| GatewayBot {
            shards: info.shards,
        })
    }

    /// `GET /guilds/{id}`: name, icon, owner, MFA level, and roles.
    /// `Refused(UnknownGuild)` (or `MissingAccess`) when Quack is not in it.
    pub async fn guild(&self, guild: GuildId) -> Outcome<Guild> {
        self.json::<twilight_model::guild::Guild>(Request::new(Route::guild(guild)))
            .await
            .and_then(|g| converted(convert::guild(&g)))
    }

    /// The server's facts from REST: the server, then Quack's own member
    /// (`quack: None` when Discord says Quack is not a member).
    pub async fn guild_facts(
        &self,
        guild: GuildId,
        quack: AccountId,
        now: OffsetDateTime,
    ) -> Outcome<GuildFacts> {
        let info = match self.guild(guild).await.into_result() {
            Ok(info) => info,
            Err(failure) => return failure,
        };
        let mut facts = GuildFacts {
            guild_id: info.id,
            name: info.name,
            icon_hash: info.icon_hash,
            owner_id: info.owner_id,
            mfa_required: info.mfa_required,
            roles: info.roles,
            quack: None,
        };
        self.member(guild, quack.member()).await.map(|member| {
            facts.quack = member.map(|m| member_facts(&facts, &m, now));
            facts
        })
    }

    /// `GET /guilds/{g}/members/{m}`: `Done(None)` only for Discord's 404
    /// Unknown Member, so "not a member" is never guessed.
    pub async fn member(&self, guild: GuildId, member: MemberId) -> Outcome<Option<Member>> {
        match self
            .json::<twilight_model::guild::Member>(Request::new(Route::member(
                Method::Get,
                guild,
                member,
            )))
            .await
        {
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownMember => Outcome::Done(None),
            other => other.and_then(|m| converted(convert::member(&m)).map(Some)),
        }
    }

    /// `GET /guilds/{g}/members/search`: members whose username or nickname
    /// starts with `query` (at most `limit`, 1–1,000).
    pub async fn search_members(
        &self,
        guild: GuildId,
        query: &str,
        limit: u16,
    ) -> Outcome<Vec<Member>> {
        let limit = u8::try_from(limit.clamp(1, 100)).unwrap_or(100);
        self.json::<Vec<twilight_model::guild::Member>>(Request::new(Route::search_members(
            guild, query, limit,
        )))
        .await
        .map(|members| members.iter().filter_map(convert::member).collect())
    }

    /// `GET /guilds/{g}/bans/{m}`: the ban and its reason, or `Done(None)`
    /// for Discord's 404 Unknown Ban.
    pub async fn get_ban(&self, guild: GuildId, member: MemberId) -> Outcome<Option<BanFacts>> {
        match self
            .json::<twilight_model::guild::Ban>(Request::new(Route::ban(
                Method::Get,
                guild,
                member,
            )))
            .await
        {
            Outcome::Refused(r) if r.kind == RefusalKind::UnknownBan => Outcome::Done(None),
            other => other.map(|ban| Some(BanFacts { reason: ban.reason })),
        }
    }

    /// Times a member out until `until` (`PATCH /guilds/{g}/members/{m}`
    /// with `communication_disabled_until`), sending `reason` as the
    /// audit-log reason. The same `until` is sent on every attempt (§5.2).
    pub async fn timeout(
        &self,
        guild: GuildId,
        member: MemberId,
        until: OffsetDateTime,
        reason: &str,
    ) -> Outcome<()> {
        let Ok(until) = until.format(&Rfc3339) else {
            return Outcome::NotSent(NotSentCause::Invalid);
        };
        self.set_timeout(guild, member, Some(until), reason).await
    }

    /// Ends a member's timeout (`communication_disabled_until: null`).
    pub async fn remove_timeout(
        &self,
        guild: GuildId,
        member: MemberId,
        reason: &str,
    ) -> Outcome<()> {
        self.set_timeout(guild, member, None, reason).await
    }

    async fn set_timeout(
        &self,
        guild: GuildId,
        member: MemberId,
        until: Option<String>,
        reason: &str,
    ) -> Outcome<()> {
        let body = TimeoutBody {
            communication_disabled_until: until,
        };
        self.unit(Request {
            reason: Some(reason),
            ..with_json(Route::member(Method::Patch, guild, member), &body)
        })
        .await
    }

    /// Kicks a member (`DELETE /guilds/{g}/members/{m}`).
    pub async fn kick(&self, guild: GuildId, member: MemberId, reason: &str) -> Outcome<()> {
        self.unit(Request {
            reason: Some(reason),
            ..Request::new(Route::member(Method::Delete, guild, member))
        })
        .await
    }

    /// Bans a member (`PUT /guilds/{g}/bans/{m}`), deleting their messages
    /// from the last `delete_message_seconds` (at most 604,800).
    pub async fn ban_member(
        &self,
        guild: GuildId,
        member: MemberId,
        delete_message_seconds: u32,
        reason: &str,
    ) -> Outcome<()> {
        let body = BanBody {
            delete_message_seconds: delete_message_seconds.min(604_800),
        };
        self.unit(Request {
            reason: Some(reason),
            ..with_json(Route::ban(Method::Put, guild, member), &body)
        })
        .await
    }

    /// Lifts a ban (`DELETE /guilds/{g}/bans/{m}`).
    pub async fn unban(&self, guild: GuildId, member: MemberId, reason: &str) -> Outcome<()> {
        self.unit(Request {
            reason: Some(reason),
            ..Request::new(Route::ban(Method::Delete, guild, member))
        })
        .await
    }
}
