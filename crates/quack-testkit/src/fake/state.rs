//! The fake's world: what Discord would hold.

use std::collections::{BTreeMap, HashMap, HashSet};

use quack_core::ids::{AccountId, AttachmentId, ChannelId, GuildId, MemberId, MessageId, RoleId};
use quack_core::permissions::{
    AccountFacts, BanFacts, ChannelFacts, GuildFacts, MemberFacts, Overwrite, OverwriteTarget,
    Permission, Permissions, RoleFacts,
};
use quack_engine::port::{FetchedAttachment, FetchedMessage};
use quack_text::Msg;
use time::OffsetDateTime;

/// Discord's channel type of a private thread.
pub const PRIVATE_THREAD: u8 = 12;

/// What @everyone may do in the fake's servers.
pub fn everyone_permissions() -> Permissions {
    Permission::ViewChannel | Permission::SendMessages | Permission::ReadMessageHistory
}

/// What one server holds.
#[derive(Clone, Debug)]
pub struct GuildState {
    /// The server's facts (owner, roles, Quack's member).
    pub facts: GuildFacts,
    /// Its members.
    pub members: HashMap<MemberId, MemberFacts>,
    /// Its bans.
    pub bans: HashMap<MemberId, BanFacts>,
    /// Its channels and threads.
    pub channels: HashMap<ChannelId, ChannelFacts>,
}

/// A message as the fake keeps it.
#[derive(Clone, Debug)]
pub struct FakeMessage {
    /// What `fetch_message` returns.
    pub fetched: FetchedMessage,
    /// The message model, when Quack sent it.
    pub msg: Option<Msg>,
    /// The nonce Discord still remembers it by (cleared once the window
    /// passes, [`World::forget_nonces`]).
    pub nonce: Option<String>,
    /// The nonce it was created with, kept for assertions.
    pub created_with: Option<String>,
}

/// How to set up a server.
#[derive(Clone, Debug)]
pub struct GuildSetup {
    /// The server.
    pub id: GuildId,
    /// Its owner.
    pub owner: AccountId,
    /// Quack's highest role position.
    pub quack_position: u32,
    /// Quack's server permissions.
    pub quack_permissions: Permissions,
    /// Whether the server requires 2FA for moderation.
    pub mfa_required: bool,
}

/// How to set up a member.
#[derive(Clone, Debug)]
pub struct MemberSetup {
    /// The member.
    pub id: MemberId,
    /// Their highest role position.
    pub position: u32,
    /// Their server permissions.
    pub permissions: Permissions,
    /// Their roles.
    pub roles: Vec<RoleId>,
    /// Whether they are a bot.
    pub bot: bool,
    /// When they joined.
    pub joined_at: OffsetDateTime,
}

impl MemberSetup {
    /// An ordinary member at role position `position`.
    pub fn new(id: MemberId, position: u32) -> Self {
        Self {
            id,
            position,
            permissions: Permissions::NONE,
            roles: Vec::new(),
            bot: false,
            joined_at: OffsetDateTime::now_utc() - time::Duration::days(30),
        }
    }

    /// With these permissions.
    #[must_use]
    pub fn permissions(mut self, permissions: impl Into<Permissions>) -> Self {
        self.permissions = permissions.into();
        self
    }

    /// With these roles.
    #[must_use]
    pub fn roles(mut self, roles: Vec<RoleId>) -> Self {
        self.roles = roles;
        self
    }

    /// A bot account.
    #[must_use]
    pub fn bot(mut self) -> Self {
        self.bot = true;
        self
    }

    fn facts(&self) -> MemberFacts {
        MemberFacts {
            id: self.id,
            bot: self.bot,
            role_ids: self.roles.clone(),
            highest_role_position: self.position,
            permissions: self.permissions,
            joined_at: Some(self.joined_at),
            timeout_until: None,
            username: format!("user{}", self.id),
            global_name: None,
            nick: None,
        }
    }
}

/// Everything the fake holds.
#[derive(Clone, Debug)]
pub struct World {
    /// Quack's own account.
    pub quack: AccountId,
    /// Servers by id.
    pub guilds: HashMap<GuildId, GuildState>,
    /// Accounts that are in no server Quack shares (absent members).
    pub accounts: HashMap<AccountId, AccountFacts>,
    /// Every message, by id.
    pub messages: BTreeMap<MessageId, FakeMessage>,
    /// DM channels Quack opened, by account.
    pub dm_channels: HashMap<AccountId, ChannelId>,
    /// Interaction tokens that stopped working.
    pub dead_tokens: HashSet<String>,
    /// Every edit of an interaction's original response, in order.
    pub interaction_edits: Vec<(String, Msg)>,
    /// Files the CDN serves, by URL.
    pub files: HashMap<String, Vec<u8>>,
    /// Members of each thread.
    pub thread_members: HashMap<ChannelId, Vec<MemberId>>,
    /// Each thread's last `(archived, locked)` state Quack set.
    pub thread_states: HashMap<ChannelId, (bool, bool)>,
    next_id: u64,
}

impl World {
    pub(crate) fn new(quack: AccountId) -> Self {
        Self {
            quack,
            guilds: HashMap::new(),
            accounts: HashMap::new(),
            messages: BTreeMap::new(),
            dm_channels: HashMap::new(),
            dead_tokens: HashSet::new(),
            interaction_edits: Vec::new(),
            files: HashMap::new(),
            thread_members: HashMap::new(),
            thread_states: HashMap::new(),
            next_id: 900_000_000_000_000_000,
        }
    }

    /// A fresh snowflake (increasing, so later messages sort later).
    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Adds a server with Quack in it.
    pub fn add_guild(&mut self, setup: &GuildSetup) {
        let quack = MemberSetup::new(self.quack.member(), setup.quack_position)
            .permissions(setup.quack_permissions)
            .bot()
            .facts();
        let everyone = RoleFacts {
            id: setup.id.everyone_role(),
            name: "@everyone".to_owned(),
            position: 0,
            permissions: everyone_permissions(),
            managed: false,
        };
        let mut members = HashMap::new();
        members.insert(quack.id, quack.clone());
        self.guilds.insert(
            setup.id,
            GuildState {
                facts: GuildFacts {
                    guild_id: setup.id,
                    name: format!("Server {}", setup.id),
                    icon_hash: None,
                    owner_id: setup.owner,
                    mfa_required: setup.mfa_required,
                    roles: vec![everyone],
                    quack: Some(quack),
                },
                members,
                bans: HashMap::new(),
                channels: HashMap::new(),
            },
        );
    }

    /// Adds a role to a server.
    pub fn add_role(&mut self, guild: GuildId, role: RoleFacts) {
        if let Some(g) = self.guilds.get_mut(&guild) {
            g.facts.roles.push(role);
        }
    }

    /// Adds (or replaces) a member. Their server permissions include
    /// @everyone's, as Discord computes them.
    pub fn add_member(&mut self, guild: GuildId, setup: &MemberSetup) {
        if let Some(g) = self.guilds.get_mut(&guild) {
            let mut facts = setup.facts();
            facts.permissions = facts.permissions | everyone_permissions();
            g.members.insert(setup.id, facts);
        }
        self.accounts.insert(
            setup.id.account(),
            AccountFacts {
                id: setup.id.account(),
                username: format!("user{}", setup.id),
                global_name: None,
                avatar_hash: None,
                bot: setup.bot,
            },
        );
    }

    /// A member, mutably.
    pub fn member_mut(&mut self, guild: GuildId, member: MemberId) -> Option<&mut MemberFacts> {
        self.guilds.get_mut(&guild)?.members.get_mut(&member)
    }

    /// A member.
    pub fn member(&self, guild: GuildId, member: MemberId) -> Option<&MemberFacts> {
        self.guilds.get(&guild)?.members.get(&member)
    }

    /// The member's ban, if any.
    pub fn ban(&self, guild: GuildId, member: MemberId) -> Option<&BanFacts> {
        self.guilds.get(&guild)?.bans.get(&member)
    }

    /// Bans a member with this reason (as a moderator would by hand).
    pub fn set_ban(&mut self, guild: GuildId, member: MemberId, reason: Option<&str>) {
        if let Some(g) = self.guilds.get_mut(&guild) {
            g.members.remove(&member);
            g.bans.insert(
                member,
                BanFacts {
                    reason: reason.map(str::to_owned),
                },
            );
        }
    }

    /// Adds a text channel. `staff_only` denies @everyone View Channel and
    /// gives Quack what staff posts need.
    pub fn add_text_channel(&mut self, guild: GuildId, staff_only: bool) -> ChannelId {
        let id = ChannelId::new(self.next_id()).unwrap_or_else(|| unreachable!());
        let mut overwrites = Vec::new();
        if staff_only {
            overwrites.push(Overwrite {
                target: OverwriteTarget::Role(guild.everyone_role()),
                allow: Permissions::NONE,
                deny: Permission::ViewChannel.into(),
            });
        }
        let quack_permissions = Permission::ViewChannel
            | Permission::SendMessages
            | Permission::ReadMessageHistory
            | Permission::AttachFiles
            | Permission::EmbedLinks;
        if let Some(g) = self.guilds.get_mut(&guild) {
            g.channels.insert(
                id,
                ChannelFacts {
                    id,
                    guild_id: guild,
                    kind: quack_core::permissions::TEXT_CHANNEL,
                    name: format!("channel-{id}"),
                    parent_id: None,
                    overwrites,
                    quack_permissions,
                },
            );
        }
        id
    }

    /// Adds a private thread named `name` under `parent`; `None` when the
    /// parent is not a channel of any server.
    pub fn add_thread(&mut self, parent: ChannelId, name: &str) -> Option<ChannelId> {
        let id = ChannelId::new(self.next_id()).unwrap_or_else(|| unreachable!());
        let (guild, parent_facts) = self
            .guilds
            .iter_mut()
            .find_map(|(gid, g)| g.channels.get(&parent).cloned().map(|p| (*gid, p)))?;
        let g = self.guilds.get_mut(&guild)?;
        g.channels.insert(
            id,
            ChannelFacts {
                id,
                guild_id: guild,
                kind: PRIVATE_THREAD,
                name: name.to_owned(),
                parent_id: Some(parent),
                overwrites: Vec::new(),
                quack_permissions: parent_facts.quack_permissions,
            },
        );
        Some(id)
    }

    /// Threads under `parent` named `name`, oldest first.
    pub fn threads_named(&self, parent: ChannelId, name: &str) -> Vec<ChannelId> {
        let mut found: Vec<ChannelId> = self
            .guilds
            .values()
            .flat_map(|g| g.channels.values())
            .filter(|c| c.kind == PRIVATE_THREAD && c.parent_id == Some(parent) && c.name == name)
            .map(|c| c.id)
            .collect();
        found.sort_unstable();
        found
    }

    /// Deletes a channel (its messages go with it).
    pub fn delete_channel(&mut self, guild: GuildId, channel: ChannelId) {
        if let Some(g) = self.guilds.get_mut(&guild) {
            g.channels.remove(&channel);
        }
        self.thread_states.remove(&channel);
        self.thread_members.remove(&channel);
        self.messages.retain(|_, m| m.fetched.channel_id != channel);
    }

    /// A member posts a message in `guild`'s `channel`; returns its id.
    /// Like Discord's REST reads, the fetched message names no server.
    pub fn post(
        &mut self,
        _guild: GuildId,
        channel: ChannelId,
        author: AccountId,
        content: &str,
        attachments: Vec<FetchedAttachment>,
    ) -> MessageId {
        let id = MessageId::new(self.next_id()).unwrap_or_else(|| unreachable!());
        self.messages.insert(
            id,
            FakeMessage {
                created_with: None,
                fetched: FetchedMessage {
                    id,
                    channel_id: channel,
                    author_id: author,
                    author_name: format!("user{author}"),
                    author_bot: false,
                    content: content.to_owned(),
                    created_at: OffsetDateTime::now_utc(),
                    edited_at: None,
                    embeds: Vec::new(),
                    attachments,
                },
                msg: None,
                nonce: None,
            },
        );
        id
    }

    /// A file attachment served by the CDN.
    pub fn attachment(
        &mut self,
        filename: &str,
        content_type: Option<&str>,
        bytes: Vec<u8>,
    ) -> FetchedAttachment {
        let id = AttachmentId::new(self.next_id()).unwrap_or_else(|| unreachable!());
        let url = format!("https://cdn.discordapp.com/attachments/{id}/{filename}");
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.files.insert(url.clone(), bytes);
        FetchedAttachment {
            id,
            filename: filename.to_owned(),
            content_type: content_type.map(str::to_owned),
            size_bytes: size,
            url,
        }
    }

    /// Discord forgets every nonce (its window passed): a create repeated
    /// with an old nonce now creates a new message.
    pub fn forget_nonces(&mut self) {
        for m in self.messages.values_mut() {
            m.nonce = None;
        }
    }

    /// Messages Quack created with this nonce, in any channel.
    pub fn created_with(&self, nonce: &str) -> usize {
        self.messages
            .values()
            .filter(|m| m.created_with.as_deref() == Some(nonce))
            .count()
    }

    /// Messages Quack sent to a channel, oldest first.
    pub fn sent_to(&self, channel: ChannelId) -> Vec<&FakeMessage> {
        self.messages
            .values()
            .filter(|m| m.fetched.channel_id == channel && m.msg.is_some())
            .collect()
    }

    /// Messages Quack sent in DMs to an account, oldest first.
    pub fn dms_to(&self, account: AccountId) -> Vec<&FakeMessage> {
        self.dm_channels
            .get(&account)
            .map_or_default(|channel| self.sent_to(*channel))
    }
}
