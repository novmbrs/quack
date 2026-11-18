//! Twilight's wire models → this crate's plain types with quack-core ids.
//!
//! Every conversion returns `None` when an id is outside quack-core's
//! snowflake range (above `i64::MAX`), which no real Discord id is; callers
//! treat that as a malformed answer.

use quack_core::ids::{AccountId, AttachmentId, ChannelId, GuildId, MemberId, MessageId, RoleId};
use quack_core::permissions::{AccountFacts, Overwrite, OverwriteTarget, Permissions, RoleFacts};
use time::OffsetDateTime;
use twilight_model::channel::permission_overwrite::{
    PermissionOverwrite, PermissionOverwriteType,
};
use twilight_model::guild::MfaLevel;
use twilight_model::util::Timestamp;

use crate::model::{Attachment, Channel, EmbedSummary, Guild, Member, Message, ThreadState};

/// A Twilight id as a quack-core id.
pub(crate) fn id<T: TryFrom<u64>, M>(id: twilight_model::id::Id<M>) -> Option<T> {
    T::try_from(id.get()).ok()
}

/// A Discord timestamp as a `time` value.
pub(crate) fn time(t: Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(t.as_micros()) * 1_000).ok()
}

pub(crate) fn permissions(p: twilight_model::guild::Permissions) -> Permissions {
    Permissions(p.bits())
}

pub(crate) fn user(u: &twilight_model::user::User) -> Option<AccountFacts> {
    Some(AccountFacts {
        id: id::<AccountId, _>(u.id)?,
        username: u.name.clone(),
        global_name: u.global_name.clone(),
        avatar_hash: u.avatar.map(|h| h.to_string()),
        bot: u.bot,
    })
}

/// The parts of a member every payload shape shares.
pub(crate) struct MemberParts<'a> {
    pub(crate) user: &'a twilight_model::user::User,
    pub(crate) nick: Option<&'a String>,
    pub(crate) roles: &'a [twilight_model::id::Id<twilight_model::id::marker::RoleMarker>],
    pub(crate) joined_at: Option<Timestamp>,
    pub(crate) timeout_until: Option<Timestamp>,
}

pub(crate) fn member_parts(p: &MemberParts<'_>) -> Option<Member> {
    let account = user(p.user)?;
    Some(Member {
        id: account.id.member(),
        bot: account.bot,
        username: account.username,
        global_name: account.global_name,
        avatar_hash: account.avatar_hash,
        nick: p.nick.cloned(),
        role_ids: p.roles.iter().filter_map(|r| id::<RoleId, _>(*r)).collect(),
        joined_at: p.joined_at.and_then(time),
        timeout_until: p.timeout_until.and_then(time),
    })
}

pub(crate) fn member(m: &twilight_model::guild::Member) -> Option<Member> {
    member_parts(&MemberParts {
        user: &m.user,
        nick: m.nick.as_ref(),
        roles: &m.roles,
        joined_at: m.joined_at,
        timeout_until: m.communication_disabled_until,
    })
}

pub(crate) fn role(r: &twilight_model::guild::Role) -> Option<RoleFacts> {
    Some(RoleFacts {
        id: id::<RoleId, _>(r.id)?,
        name: r.name.clone(),
        position: u32::try_from(r.position.max(0)).unwrap_or(u32::MAX),
        permissions: permissions(r.permissions),
        managed: r.managed,
    })
}

pub(crate) fn guild(g: &twilight_model::guild::Guild) -> Option<Guild> {
    Some(Guild {
        id: id::<GuildId, _>(g.id)?,
        name: g.name.clone(),
        icon_hash: g.icon.map(|h| h.to_string()),
        owner_id: id::<AccountId, _>(g.owner_id)?,
        mfa_required: g.mfa_level == MfaLevel::Elevated,
        roles: g.roles.iter().filter_map(role).collect(),
    })
}

pub(crate) fn overwrite(o: &PermissionOverwrite) -> Option<Overwrite> {
    let target = match o.kind {
        PermissionOverwriteType::Member => OverwriteTarget::Member(MemberId::new(o.id.get())?),
        PermissionOverwriteType::Role => OverwriteTarget::Role(RoleId::new(o.id.get())?),
        _ => return None,
    };
    Some(Overwrite {
        target,
        allow: permissions(o.allow),
        deny: permissions(o.deny),
    })
}

pub(crate) fn channel(c: &twilight_model::channel::Channel) -> Option<Channel> {
    Some(Channel {
        id: id::<ChannelId, _>(c.id)?,
        guild_id: c.guild_id.and_then(id::<GuildId, _>),
        kind: u8::from(c.kind),
        name: c.name.clone().unwrap_or_default(),
        parent_id: c.parent_id.and_then(id::<ChannelId, _>),
        overwrites: c
            .permission_overwrites
            .iter()
            .flatten()
            .filter_map(overwrite)
            .collect(),
        thread: c.thread_metadata.as_ref().map(|t| ThreadState {
            archived: t.archived,
            locked: t.locked,
        }),
    })
}

pub(crate) fn message(m: &twilight_model::channel::Message) -> Option<Message> {
    Some(Message {
        id: id::<MessageId, _>(m.id)?,
        channel_id: id::<ChannelId, _>(m.channel_id)?,
        guild_id: m.guild_id.and_then(id::<GuildId, _>),
        author: user(&m.author)?,
        webhook: m.webhook_id.is_some(),
        member_role_ids: m
            .member
            .as_ref()
            .map(|pm| pm.roles.iter().filter_map(|r| id::<RoleId, _>(*r)).collect()),
        content: m.content.clone(),
        created_at: time(m.timestamp)?,
        edited_at: m.edited_timestamp.and_then(time),
        embeds: m
            .embeds
            .iter()
            .map(|e| EmbedSummary {
                kind: e.kind.clone(),
                title: e.title.clone(),
                description: e.description.clone(),
                url: e.url.clone(),
            })
            .collect(),
        attachments: m
            .attachments
            .iter()
            .filter_map(|a| {
                Some(Attachment {
                    id: id::<AttachmentId, _>(a.id)?,
                    filename: a.filename.clone(),
                    content_type: a.content_type.clone(),
                    size: a.size,
                    url: a.url.clone(),
                })
            })
            .collect(),
    })
}
