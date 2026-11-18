//! The fake's `Discord` implementation.

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId};
use quack_core::permissions::{AccountFacts, BanFacts, ChannelFacts, GuildFacts, MemberFacts};
use quack_engine::port::{
    BoxFuture, ChannelSpec, Discord, FetchedMessage, FileEdit, Fresh, InteractionToken, Membership,
    Nonce, Outcome, OutgoingFile, Refusal, RefusalKind, SentMessage,
};
use quack_text::Msg;
use time::OffsetDateTime;

use super::effects::{ban, edit, kick, send, set_timeout, unban};
use super::run::{about, refused, with};
use super::{Call, FakeDiscord, Recorded};

impl Discord for FakeDiscord {
    fn guild_facts(&self, guild: GuildId) -> BoxFuture<'_, Outcome<GuildFacts>> {
        let r = with(Call::GuildFacts, |r| r.guild = Some(guild));
        Box::pin(self.call(r, move |w| match w.guilds.get(&guild) {
            Some(g) => Outcome::Done(g.facts.clone()),
            None => refused(RefusalKind::UnknownGuild),
        }))
    }

    fn member(
        &self,
        guild: GuildId,
        member: MemberId,
        _fresh: Fresh,
    ) -> BoxFuture<'_, Outcome<Membership>> {
        let r = with(Call::Member, |r| {
            r.guild = Some(guild);
            r.member = Some(member);
        });
        Box::pin(self.call(r, move |w| {
            match w.guilds.get(&guild) {
                None => refused(RefusalKind::UnknownGuild),
                Some(g) => Outcome::Done(
                    g.members
                        .get(&member)
                        .cloned()
                        .map_or(Membership::Absent, Membership::Present),
                ),
            }
        }))
    }

    fn accounts<'a>(&'a self, ids: &'a [AccountId]) -> BoxFuture<'a, Outcome<Vec<AccountFacts>>> {
        let r = Recorded::new(Call::Accounts);
        Box::pin(self.call(r, move |w| {
            Outcome::Done(
                ids.iter()
                    .filter_map(|id| w.accounts.get(id).cloned())
                    .collect(),
            )
        }))
    }

    fn channel_facts(
        &self,
        guild: GuildId,
        channel: ChannelId,
    ) -> BoxFuture<'_, Outcome<Option<ChannelFacts>>> {
        let r = with(Call::ChannelFacts, |r| {
            r.guild = Some(guild);
            r.channel = Some(channel);
        });
        Box::pin(self.call(r, move |w| match w.guilds.get(&guild) {
            None => refused(RefusalKind::UnknownGuild),
            Some(g) => Outcome::Done(g.channels.get(&channel).cloned()),
        }))
    }

    fn guild_channels(&self, guild: GuildId) -> BoxFuture<'_, Outcome<Vec<ChannelFacts>>> {
        let r = with(Call::GuildChannels, |r| r.guild = Some(guild));
        Box::pin(self.call(r, move |w| match w.guilds.get(&guild) {
            None => refused(RefusalKind::UnknownGuild),
            Some(g) => Outcome::Done(g.channels.values().cloned().collect()),
        }))
    }

    fn ban(&self, guild: GuildId, member: MemberId) -> BoxFuture<'_, Outcome<Option<BanFacts>>> {
        let r = with(Call::GetBan, |r| {
            r.guild = Some(guild);
            r.member = Some(member);
        });
        Box::pin(self.call(r, move |w| match w.guilds.get(&guild) {
            None => refused(RefusalKind::UnknownGuild),
            Some(g) => Outcome::Done(g.bans.get(&member).cloned()),
        }))
    }

    fn is_thread_member(
        &self,
        thread: ChannelId,
        account: AccountId,
    ) -> BoxFuture<'_, Outcome<bool>> {
        let r = with(Call::IsThreadMember, |r| r.channel = Some(thread));
        Box::pin(self.call(r, move |w| {
            Outcome::Done(
                w.thread_members
                    .get(&thread)
                    .is_some_and(|m| m.contains(&account.member())),
            )
        }))
    }

    fn search_members<'a>(
        &'a self,
        guild: GuildId,
        query: &'a str,
        limit: u8,
    ) -> BoxFuture<'a, Outcome<Vec<MemberFacts>>> {
        let r = with(Call::SearchMembers, |r| r.guild = Some(guild));
        Box::pin(self.call(r, move |w| {
            match w.guilds.get(&guild) {
                None => refused(RefusalKind::UnknownGuild),
                Some(g) => Outcome::Done(
                    g.members
                        .values()
                        .filter(|m| m.username.starts_with(query))
                        .take(usize::from(limit))
                        .cloned()
                        .collect(),
                ),
            }
        }))
    }

    fn timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        until: OffsetDateTime,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        let mut r = about(Call::Timeout, guild, member, reason);
        r.until = Some(until);
        Box::pin(self.call(r, move |w| set_timeout(w, guild, member, Some(until))))
    }

    fn remove_timeout<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        let r = about(Call::RemoveTimeout, guild, member, reason);
        Box::pin(self.call(r, move |w| set_timeout(w, guild, member, None)))
    }

    fn kick<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        let r = about(Call::Kick, guild, member, reason);
        Box::pin(self.call(r, move |w| kick(w, guild, member)))
    }

    fn ban_member<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        delete_message_seconds: u32,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        let mut r = about(Call::Ban, guild, member, reason);
        r.reason = Some(format!("{reason} (delete {delete_message_seconds}s)"));
        Box::pin(self.call(r, move |w| ban(w, guild, member, reason)))
    }

    fn unban<'a>(
        &'a self,
        guild: GuildId,
        member: MemberId,
        reason: &'a str,
    ) -> BoxFuture<'a, Outcome<()>> {
        let r = about(Call::Unban, guild, member, reason);
        Box::pin(self.call(r, move |w| unban(w, guild, member)))
    }

    fn open_dm(&self, account: AccountId) -> BoxFuture<'_, Outcome<ChannelId>> {
        let r = with(Call::OpenDm, |r| r.member = Some(account.member()));
        Box::pin(self.call(r, move |w| {
            if let Some(channel) = w.dm_channels.get(&account) {
                return Outcome::Done(*channel);
            }
            let channel = ChannelId::new(w.next_id()).unwrap_or_else(|| unreachable!());
            w.dm_channels.insert(account, channel);
            Outcome::Done(channel)
        }))
    }

    fn send<'a>(
        &'a self,
        channel: ChannelId,
        msg: &'a Msg,
        files: &'a [OutgoingFile],
        nonce: &'a Nonce,
    ) -> BoxFuture<'a, Outcome<SentMessage>> {
        let r = with(Call::Send, |r| {
            r.channel = Some(channel);
            r.nonce = Some(nonce.clone());
            r.msg = Some(msg.clone());
        });
        Box::pin(self.call(r, move |w| send(w, channel, msg, files, nonce)))
    }

    fn edit<'a>(
        &'a self,
        channel: ChannelId,
        message: MessageId,
        msg: &'a Msg,
        files: &'a FileEdit,
    ) -> BoxFuture<'a, Outcome<SentMessage>> {
        let r = with(Call::Edit, |r| {
            r.channel = Some(channel);
            r.message = Some(message);
            r.msg = Some(msg.clone());
        });
        Box::pin(self.call(r, move |w| edit(w, channel, message, msg, files)))
    }

    fn edit_interaction_original<'a>(
        &'a self,
        token: &'a InteractionToken,
        msg: &'a Msg,
    ) -> BoxFuture<'a, Outcome<()>> {
        let r = with(Call::EditOriginal, |r| r.msg = Some(msg.clone()));
        Box::pin(self.call(r, move |w| {
            if w.dead_tokens.contains(token.expose()) {
                return Outcome::Refused(Refusal {
                    status: 404,
                    code: Some(10_015),
                    kind: RefusalKind::Other,
                });
            }
            w.interaction_edits
                .push((token.expose().to_owned(), msg.clone()));
            Outcome::Done(())
        }))
    }

    fn delete_message(&self, channel: ChannelId, message: MessageId) -> BoxFuture<'_, Outcome<()>> {
        let r = with(Call::DeleteMessage, |r| {
            r.channel = Some(channel);
            r.message = Some(message);
        });
        Box::pin(self.call(r, move |w| match w.messages.get(&message) {
            Some(m) if m.fetched.channel_id == channel => {
                w.messages.remove(&message);
                Outcome::Done(())
            }
            _ => refused(RefusalKind::UnknownMessage),
        }))
    }

    fn fetch_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> BoxFuture<'_, Outcome<FetchedMessage>> {
        let r = with(Call::FetchMessage, |r| {
            r.channel = Some(channel);
            r.message = Some(message);
        });
        Box::pin(self.call(r, move |w| match w.messages.get(&message) {
            Some(m) if m.fetched.channel_id == channel => Outcome::Done(m.fetched.clone()),
            _ => refused(RefusalKind::UnknownMessage),
        }))
    }

    fn fetch_history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u8,
    ) -> BoxFuture<'_, Outcome<Vec<FetchedMessage>>> {
        let r = with(Call::FetchHistory, |r| r.channel = Some(channel));
        Box::pin(self.call(r, move |w| {
            Outcome::Done(
                w.messages
                    .values()
                    .rev()
                    .filter(|m| m.fetched.channel_id == channel)
                    .filter(|m| before.is_none_or(|b| m.fetched.id < b))
                    .take(usize::from(limit))
                    .map(|m| m.fetched.clone())
                    .collect(),
            )
        }))
    }

    fn download_attachment<'a>(
        &'a self,
        url: &'a str,
        max_bytes: u64,
    ) -> BoxFuture<'a, Outcome<Vec<u8>>> {
        let r = Recorded::new(Call::Download);
        Box::pin(self.call(r, move |w| match w.files.get(url) {
            Some(bytes) if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max_bytes => {
                Outcome::Done(bytes.clone())
            }
            Some(_) => refused(RefusalKind::TooLarge),
            None => refused(RefusalKind::Other),
        }))
    }

    fn create_channel<'a>(
        &'a self,
        guild: GuildId,
        spec: &'a ChannelSpec,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        let r = with(Call::CreateChannel, |r| r.guild = Some(guild));
        Box::pin(self.call(r, move |w| {
            let quack = w.quack;
            let id = w.add_text_channel(guild, false);
            if let Some(c) = w
                .guilds
                .get_mut(&guild)
                .and_then(|g| g.channels.get_mut(&id))
            {
                c.name.clone_from(&spec.name);
                c.overwrites.clone_from(&spec.overwrites);
                // What Quack's own member overwrite allows, it holds there.
                for o in &spec.overwrites {
                    if o.target == quack_core::permissions::OverwriteTarget::Member(quack.member())
                    {
                        c.quack_permissions = c.quack_permissions | o.allow;
                    }
                }
            }
            Outcome::Done(id)
        }))
    }

    fn create_private_thread<'a>(
        &'a self,
        parent: ChannelId,
        name: &'a str,
    ) -> BoxFuture<'a, Outcome<ChannelId>> {
        let r = with(Call::CreateThread, |r| r.channel = Some(parent));
        Box::pin(self.call(r, move |w| match w.add_thread(parent, name) {
            Some(id) => Outcome::Done(id),
            None => refused(RefusalKind::UnknownChannel),
        }))
    }

    fn find_threads<'a>(
        &'a self,
        guild: GuildId,
        parent: ChannelId,
        name: &'a str,
    ) -> BoxFuture<'a, Outcome<Vec<ChannelId>>> {
        let r = with(Call::FindThreads, |r| {
            r.guild = Some(guild);
            r.channel = Some(parent);
        });
        Box::pin(self.call(r, move |w| Outcome::Done(w.threads_named(parent, name))))
    }

    fn add_thread_member(&self, thread: ChannelId, member: MemberId) -> BoxFuture<'_, Outcome<()>> {
        let r = with(Call::AddThreadMember, |r| r.channel = Some(thread));
        Box::pin(self.call(r, move |w| {
            let members = w.thread_members.entry(thread).or_default();
            if !members.contains(&member) {
                members.push(member);
            }
            Outcome::Done(())
        }))
    }

    fn remove_thread_member(
        &self,
        thread: ChannelId,
        member: MemberId,
    ) -> BoxFuture<'_, Outcome<()>> {
        let r = with(Call::RemoveThreadMember, |r| r.channel = Some(thread));
        Box::pin(self.call(r, move |w| {
            if let Some(members) = w.thread_members.get_mut(&thread) {
                members.retain(|m| *m != member);
            }
            Outcome::Done(())
        }))
    }

    fn thread_members(&self, thread: ChannelId) -> BoxFuture<'_, Outcome<Vec<MemberId>>> {
        let r = with(Call::ThreadMembers, |r| r.channel = Some(thread));
        Box::pin(self.call(r, move |w| {
            Outcome::Done(w.thread_members.get(&thread).cloned().unwrap_or_default())
        }))
    }

    fn set_thread_state(
        &self,
        thread: ChannelId,
        archived: bool,
        locked: bool,
    ) -> BoxFuture<'_, Outcome<()>> {
        let r = with(Call::SetThreadState, |r| r.channel = Some(thread));
        Box::pin(self.call(r, move |w| {
            let exists = w.guilds.values().any(|g| g.channels.contains_key(&thread));
            if !exists {
                return refused(RefusalKind::UnknownChannel);
            }
            w.thread_states.insert(thread, (archived, locked));
            Outcome::Done(())
        }))
    }

    fn delete_channel(&self, channel: ChannelId) -> BoxFuture<'_, Outcome<()>> {
        let r = with(Call::DeleteChannel, |r| r.channel = Some(channel));
        Box::pin(self.call(r, move |w| {
            let guild = w
                .guilds
                .iter()
                .find(|(_, g)| g.channels.contains_key(&channel))
                .map(|(id, _)| *id);
            match guild {
                Some(guild) => {
                    w.delete_channel(guild, channel);
                    Outcome::Done(())
                }
                None => refused(RefusalKind::UnknownChannel),
            }
        }))
    }
}
