//! A ready test world: a fresh database, the fake Discord with one server
//! and its people, and an engine over both.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use quack_core::case::ContextInput;
use quack_core::ids::{AccountId, CaseId, GuildId, MemberId, RuleId};
use quack_core::permissions::{Permission, Permissions};
use quack_core::rule::{Level, LevelAction, RuleDefinition};
use quack_db::guilds::{self, GuildFacts as DbGuildFacts};
use quack_db::rules::{self, NewRule};
use quack_engine::cases::{Actor, Applied, ApplyRule, CaseCreated, apply_rule};
use quack_engine::{Engine, EngineConfig, OpKey, Origin, Principal};

use crate::db::TestDb;
use crate::fake::{FakeDiscord, GuildSetup, MemberSetup};

/// The standard server.
pub const GUILD: u64 = 1001;
/// Its owner.
pub const OWNER: u64 = 10;
/// Quack's account.
pub const QUACK: u64 = 20;
/// A moderator (Moderate Members, role position 30).
pub const MODERATOR: u64 = 30;
/// A second moderator (role position 30).
pub const MODERATOR_2: u64 = 31;
/// An administrator (role position 40).
pub const ADMIN: u64 = 50;
/// A member (role position 5).
pub const MEMBER: u64 = 40;
/// Another member (role position 5).
pub const MEMBER_2: u64 = 41;

/// A snowflake from a test constant.
pub fn sf<T: TryFrom<u64>>(n: u64) -> T {
    T::try_from(n).unwrap_or_else(|_| unreachable!("test ids are positive"))
}

/// Everything Quack needs in the standard server.
pub fn quack_permissions() -> Permissions {
    [
        Permission::ModerateMembers,
        Permission::KickMembers,
        Permission::BanMembers,
        Permission::ViewChannel,
        Permission::SendMessages,
        Permission::ReadMessageHistory,
        Permission::AttachFiles,
        Permission::EmbedLinks,
        Permission::ManageMessages,
        Permission::ManageChannels,
        Permission::ManageThreads,
        Permission::CreatePrivateThreads,
        Permission::SendMessagesInThreads,
        Permission::ViewAuditLog,
    ]
    .into_iter()
    .collect()
}

/// A test world.
#[derive(Debug)]
pub struct Harness {
    /// The database.
    pub db: TestDb,
    /// The fake Discord.
    pub fake: Arc<FakeDiscord>,
    /// The engine over both.
    pub engine: Engine,
    /// The standard server.
    pub guild: GuildId,
    next_key: std::sync::atomic::AtomicU64,
}

impl Harness {
    /// A fresh world with the standard server installed.
    pub async fn new() -> Result<Self> {
        Self::with_connections(16).await
    }

    /// A fresh world with a pool of `connections`.
    pub async fn with_connections(connections: u32) -> Result<Self> {
        let db = TestDb::with_connections(connections).await?;
        let fake = Arc::new(FakeDiscord::new(sf(QUACK)));
        let guild: GuildId = sf(GUILD);
        fake.world(|w| {
            w.add_guild(&GuildSetup {
                id: guild,
                owner: sf(OWNER),
                quack_position: 50,
                quack_permissions: quack_permissions(),
                mfa_required: false,
            });
            w.add_member(guild, &MemberSetup::new(sf(OWNER), 60));
            w.add_member(
                guild,
                &MemberSetup::new(sf(MODERATOR), 30).permissions(Permission::ModerateMembers),
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(MODERATOR_2), 30).permissions(Permission::ModerateMembers),
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(ADMIN), 40).permissions(Permission::Administrator),
            );
            w.add_member(guild, &MemberSetup::new(sf(MEMBER), 5));
            w.add_member(guild, &MemberSetup::new(sf(MEMBER_2), 5));
        });
        let engine = Engine::new(
            db.db.clone(),
            fake.clone(),
            EngineConfig {
                public_url: "https://quack.test".to_owned(),
                session_key_fingerprint: [0; 8],
            },
        );
        let h = Self {
            db,
            fake,
            engine,
            guild,
            next_key: std::sync::atomic::AtomicU64::new(1),
        };
        h.install(guild).await?;
        Ok(h)
    }

    /// Installs a server in the database (its rows and child rows).
    pub async fn install(&self, guild: GuildId) -> Result<()> {
        let mut tx = self.db.db.begin().await?;
        guilds::upsert(
            &mut tx,
            &DbGuildFacts {
                id: guild,
                name: format!("Server {guild}"),
                icon_hash: None,
                owner_id: sf(OWNER),
                mfa_required: false,
            },
        )
        .await?;
        guilds::ensure_children(&mut tx, guild).await?;
        tx.commit().await?;
        Ok(())
    }

    /// A fresh operation key.
    pub fn key(&self, operation: &str) -> OpKey {
        let n = self
            .next_key
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        OpKey::new(operation, format!("key-{n}"))
    }

    /// A staff principal for `account` in the standard server, from the
    /// fake's current facts.
    pub fn principal(&self, account: u64, origin: Origin) -> Principal {
        self.principal_mfa(account, origin, false)
    }

    /// A staff principal with Quack's 2FA record set to `mfa`.
    pub fn principal_mfa(&self, account: u64, origin: Origin, mfa: bool) -> Principal {
        let account: AccountId = sf(account);
        self.fake.world(|w| {
            let g = w.guilds.get(&self.guild).map(|g| g.facts.clone());
            let member = w.member(self.guild, account.member()).cloned();
            let Some(g) = g else {
                unreachable!("the standard server exists")
            };
            Principal::from_facts(&g, account, origin, member.as_ref(), mfa)
        })
    }

    /// The standard moderator, acting from Discord.
    pub fn moderator(&self) -> Principal {
        self.principal(MODERATOR, Origin::Discord)
    }

    /// Creates a rule in the server (as Quack) and returns its id.
    pub async fn rule(&self, definition: RuleDefinition) -> Result<RuleId> {
        let id = RuleId::from_uuid(uuid::Uuid::now_v7());
        let mut tx = self.db.db.begin().await?;
        rules::create(
            &mut tx,
            &NewRule {
                id,
                guild: self.guild,
                definition,
                created_by: None,
                reconstructed: false,
            },
        )
        .await?;
        tx.commit().await?;
        self.engine
            .rules_cache()
            .reload(&self.db.db, self.guild)
            .await?;
        Ok(id)
    }

    /// An application of `rule` to `member` by the standard moderator from
    /// the dashboard, with no context and no receipt.
    pub fn request(&self, member: u64, rule: RuleId) -> ApplyRule {
        ApplyRule {
            guild: self.guild,
            actor: Actor::Staff(self.principal(MODERATOR, Origin::Dashboard)),
            member: sf::<MemberId>(member),
            rule,
            pinned_version: None,
            context: Vec::<(String, ContextInput)>::new(),
            evidence_links: Vec::new(),
            source_message: None,
            receipt: None,
            replaces: None,
            op_key: self.key("POST /api/guilds/{guild_id}/cases"),
            draft: None,
            honeypot: None,
        }
    }

    /// Applies `rule` to `member` and expects a new case.
    pub async fn apply(&self, member: u64, rule: RuleId) -> Result<CaseCreated> {
        match apply_rule(&self.engine, self.request(member, rule)).await? {
            Applied::Created(created) => Ok(*created),
            Applied::Replayed(_) => anyhow::bail!("expected a new case"),
        }
    }

    /// Moves a case's creation time back (the DM wait, counting windows).
    pub async fn age_case(&self, case: CaseId, seconds: u64) -> Result<()> {
        self.db
            .execute(&format!(
                "UPDATE cases SET created_at = created_at - interval '{seconds} seconds' WHERE id = '{}'",
                case.get()
            ))
            .await
            .context("age case")?;
        Ok(())
    }
}

/// Rule definitions for tests.
pub mod rules_for_tests {
    use super::{Level, LevelAction, RuleDefinition};

    /// A rule named `name` with one level per entry, starting at 1, 2, 3…
    pub fn levels(name: &str, actions: &[Option<LevelAction>], send_dm: bool) -> RuleDefinition {
        RuleDefinition {
            name: name.to_owned(),
            description: String::new(),
            reason: "Please follow the server rules.".to_owned(),
            appealable: true,
            counting_window_days: None,
            fields: Vec::new(),
            levels: actions
                .iter()
                .enumerate()
                .map(|(i, action)| Level {
                    from_case_count: u32::try_from(i + 1).unwrap_or(u32::MAX),
                    action: *action,
                    send_dm,
                })
                .collect(),
        }
    }

    /// A warning-only rule.
    pub fn warning(name: &str) -> RuleDefinition {
        levels(name, &[None], true)
    }

    /// A one-level rule taking `action`.
    pub fn one(name: &str, action: LevelAction, send_dm: bool) -> RuleDefinition {
        levels(name, &[Some(action)], send_dm)
    }

    /// A one-hour timeout.
    pub const TIMEOUT: LevelAction = LevelAction::Timeout {
        duration_seconds: 3600,
    };
    /// A kick.
    pub const KICK: LevelAction = LevelAction::Kick;
    /// A ban deleting nothing.
    pub const BAN: LevelAction = LevelAction::Ban {
        delete_message_seconds: 0,
    };
}
