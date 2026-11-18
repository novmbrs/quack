//! What the fake's moderation and message calls do to the world.

use quack_core::ids::{AttachmentId, ChannelId, GuildId, MemberId, MessageId};
use quack_engine::port::{
    FetchedAttachment, FetchedMessage, FileEdit, Nonce, Outcome, OutgoingFile, RefusalKind,
    SentMessage,
};
use quack_text::Msg;
use time::OffsetDateTime;

use super::run::refused;
use super::state::{FakeMessage, World};

/// A create with `enforce_nonce`: a nonce already used in the channel
/// returns that message instead of creating another.
pub(super) fn send(
    w: &mut World,
    channel: ChannelId,
    msg: &Msg,
    files: &[OutgoingFile],
    nonce: &Nonce,
) -> Outcome<SentMessage> {
    let existing = w
        .messages
        .values()
        .find(|m| m.fetched.channel_id == channel && m.nonce.as_deref() == Some(nonce.as_str()));
    if let Some(m) = existing {
        return Outcome::Done(SentMessage {
            channel_id: channel,
            message_id: m.fetched.id,
            attachments: m
                .fetched
                .attachments
                .iter()
                .map(|a| (a.filename.clone(), a.id))
                .collect(),
        });
    }
    let dm = w.dm_channels.values().any(|c| *c == channel);
    let guild = w
        .guilds
        .iter()
        .find(|(_, g)| g.channels.contains_key(&channel))
        .map(|(id, _)| *id);
    if guild.is_none() && !dm {
        return refused(RefusalKind::UnknownChannel);
    }
    let id = MessageId::new(w.next_id()).unwrap_or_else(|| unreachable!());
    let attachments: Vec<FetchedAttachment> = files
        .iter()
        .map(|f| {
            let aid = AttachmentId::new(w.next_id()).unwrap_or_else(|| unreachable!());
            FetchedAttachment {
                id: aid,
                filename: f.name.clone(),
                content_type: f.content_type.clone(),
                size_bytes: u64::try_from(f.bytes.len()).unwrap_or(u64::MAX),
                url: format!("https://cdn.discordapp.com/attachments/{aid}/{}", f.name),
            }
        })
        .collect();
    let sent = SentMessage {
        channel_id: channel,
        message_id: id,
        attachments: attachments
            .iter()
            .map(|a| (a.filename.clone(), a.id))
            .collect(),
    };
    w.messages.insert(
        id,
        FakeMessage {
            fetched: FetchedMessage {
                id,
                channel_id: channel,
                guild_id: guild,
                author_id: w.quack,
                author_name: "Quack".to_owned(),
                author_bot: true,
                content: String::new(),
                created_at: OffsetDateTime::now_utc(),
                edited_at: None,
                embeds: Vec::new(),
                attachments,
            },
            msg: Some(msg.clone()),
            nonce: Some(nonce.as_str().to_owned()),
        },
    );
    Outcome::Done(sent)
}

/// An edit of one of Quack's messages; a replaced file keeps a stable id.
pub(super) fn edit(
    w: &mut World,
    channel: ChannelId,
    message: MessageId,
    msg: &Msg,
    files: &FileEdit,
) -> Outcome<SentMessage> {
    let Some(found) = w.messages.get_mut(&message) else {
        return refused(RefusalKind::UnknownMessage);
    };
    if found.fetched.channel_id != channel {
        return refused(RefusalKind::UnknownMessage);
    }
    found.msg = Some(msg.clone());
    found.fetched.edited_at = Some(OffsetDateTime::now_utc());
    let attachments = match files {
        FileEdit::Replace(f) => vec![(f.name.clone(), first_attachment(message))],
        FileEdit::Keep(a) => vec![(String::new(), *a)],
        FileEdit::None => Vec::new(),
    };
    Outcome::Done(SentMessage {
        channel_id: channel,
        message_id: message,
        attachments,
    })
}

fn first_attachment(message: MessageId) -> AttachmentId {
    AttachmentId::new(message.get()).unwrap_or_else(|| unreachable!())
}

/// Places or (with `None`) removes a timeout.
pub(super) fn set_timeout(
    w: &mut World,
    guild: GuildId,
    member: MemberId,
    until: Option<OffsetDateTime>,
) -> Outcome<()> {
    match w.member_mut(guild, member) {
        Some(m) => {
            m.timeout_until = until;
            Outcome::Done(())
        }
        None => refused(RefusalKind::UnknownMember),
    }
}

/// Removes a present member.
pub(super) fn kick(w: &mut World, guild: GuildId, member: MemberId) -> Outcome<()> {
    let gone = w
        .guilds
        .get_mut(&guild)
        .and_then(|g| g.members.remove(&member));
    if gone.is_some() {
        Outcome::Done(())
    } else {
        refused(RefusalKind::UnknownMember)
    }
}

/// Bans an account (present or not) with `reason`.
pub(super) fn ban(w: &mut World, guild: GuildId, member: MemberId, reason: &str) -> Outcome<()> {
    if !w.guilds.contains_key(&guild) {
        return refused(RefusalKind::UnknownGuild);
    }
    w.set_ban(guild, member, Some(reason));
    Outcome::Done(())
}

/// Lifts a ban.
pub(super) fn unban(w: &mut World, guild: GuildId, member: MemberId) -> Outcome<()> {
    let removed = w
        .guilds
        .get_mut(&guild)
        .and_then(|g| g.bans.remove(&member));
    if removed.is_some() {
        Outcome::Done(())
    } else {
        refused(RefusalKind::UnknownBan)
    }
}
