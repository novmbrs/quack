//! Shared set-up for the honeypot scenario tests: a server with a honeypot
//! channel and rule, turned on.

#![allow(dead_code, unreachable_pub, missing_docs)]

use anyhow::Result;
use quack_core::ids::{ChannelId, GuildId, MessageId, RuleId};
use quack_core::modlog::ModLogKind;
use quack_core::permissions::{ActorFacts, Permission, Permissions};
use quack_db::modlog::{self, ModLogFilter};
use quack_engine::settings::set_enabled;
use quack_engine::{Origin, Principal};
use quack_honeypot::{HoneypotSettingsInput, Intake, PostFacts};
use quack_testkit::rules_for_tests::{TIMEOUT, one};
use quack_testkit::{ADMIN, Harness, sf};

pub struct World {
    pub h: Harness,
    pub channel: ChannelId,
    pub rule: RuleId,
}

pub fn admin(h: &Harness) -> Principal {
    h.principal(ADMIN, Origin::Dashboard)
}

pub fn trap_permissions() -> Permissions {
    [
        Permission::ViewChannel,
        Permission::SendMessages,
        Permission::ReadMessageHistory,
        Permission::ManageMessages,
    ]
    .into_iter()
    .collect()
}

/// A server with the channel and rule saved, still off.
pub async fn configured() -> Result<World> {
    let h = Harness::new().await?;
    h.register(quack_honeypot::jobs(h.engine.clone()));
    let channel = h.fake.world(|w| {
        let c = w.add_text_channel(h.guild, false);
        if let Some(f) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&c))
        {
            f.quack_permissions = trap_permissions();
        }
        c
    });
    let rule = h.rule(one("Spam", TIMEOUT, false)).await?;
    let w = World { h, channel, rule };
    w.save(Some(channel), Some(rule), None).await?;
    Ok(w)
}

/// A server with the honeypot on.
pub async fn world() -> Result<World> {
    let w = configured().await?;
    w.enable(true).await?;
    w.h.settle().await?;
    Ok(w)
}

impl World {
    pub async fn save(
        &self,
        channel: Option<ChannelId>,
        rule: Option<RuleId>,
        warning: Option<&str>,
    ) -> Result<quack_honeypot::HoneypotSettingsView> {
        let current = quack_honeypot::settings(&self.h.engine, &admin(&self.h)).await?;
        Ok(quack_honeypot::update_settings(
            &self.h.engine,
            &admin(&self.h),
            &HoneypotSettingsInput {
                expected_version: current.version,
                channel_id: channel,
                rule_id: rule,
                warning_text: warning.map(str::to_owned),
            },
            &self.h.key("PUT"),
        )
        .await?)
    }

    pub async fn enable(&self, on: bool) -> Result<()> {
        set_enabled(
            &self.h.engine,
            &admin(&self.h),
            &quack_honeypot::hooks(),
            on,
            &self.h.key("enable"),
        )
        .await?;
        // The binary reloads on NOTIFY; tests reload by hand.
        self.h
            .engine
            .config_cache()
            .reload(&self.h.db.db, self.h.guild)
            .await?;
        Ok(())
    }

    /// `author` posts in `channel`; the gateway hands it to the module.
    pub async fn post_in(
        &self,
        guild: GuildId,
        channel: ChannelId,
        author: u64,
    ) -> Result<(MessageId, Intake)> {
        let id = self
            .h
            .fake
            .world(|w| w.post(guild, channel, sf(author), "free nitro", Vec::new()));
        let facts = self.facts(guild, id, channel, author, false, false);
        Ok((
            id,
            quack_honeypot::on_message(&self.h.engine, &facts).await?,
        ))
    }

    pub async fn post(&self, author: u64) -> Result<(MessageId, Intake)> {
        self.post_in(self.h.guild, self.channel, author).await
    }

    pub fn facts(
        &self,
        guild: GuildId,
        message: MessageId,
        channel: ChannelId,
        author: u64,
        bot: bool,
        webhook: bool,
    ) -> PostFacts {
        let (owner, member) = self.h.fake.world(|w| {
            let owner = w.guilds.get(&guild).map(|g| g.facts.owner_id);
            (owner, w.member(guild, sf(author)).cloned())
        });
        PostFacts {
            guild,
            channel,
            message,
            author: sf(author),
            author_bot: bot || member.as_ref().is_some_and(|m| m.bot),
            webhook,
            actor: ActorFacts {
                is_owner: owner == Some(sf(author)),
                permissions: member.as_ref().map_or(Permissions::NONE, |m| m.permissions),
                role_ids: member.map_or_default(|m| m.role_ids),
                mfa_enabled: false,
            },
        }
    }

    pub fn exists(&self, message: MessageId) -> bool {
        self.h.fake.world(|w| w.messages.contains_key(&message))
    }

    pub async fn failures(&self) -> Result<Vec<String>> {
        let rows = self
            .h
            .db
            .count("SELECT count(*) FROM honeypot_hits WHERE state = 'failed'")
            .await?;
        let mut out = Vec::new();
        for code in [
            "exempt",
            "policy_changed",
            "rule_unavailable",
            "discord_unavailable",
            "quack_role_too_low",
            "quack_not_in_server",
        ] {
            let n = self
                .h
                .db
                .count(&format!(
                    "SELECT count(*) FROM honeypot_hits WHERE failure = '{code}'"
                ))
                .await?;
            for _ in 0..n {
                out.push(code.to_owned());
            }
        }
        assert_eq!(out.len(), usize::try_from(rows)?, "every failure is known");
        Ok(out)
    }

    pub async fn cases(&self) -> Result<i64> {
        self.h
            .db
            .count("SELECT count(*) FROM cases WHERE source = 'honeypot'")
            .await
    }

    pub async fn log_kinds(&self) -> Result<Vec<ModLogKind>> {
        let mut conn = self.h.db.db.conn().await?;
        let entries =
            modlog::list(&mut conn, self.h.guild, &ModLogFilter::default(), None, 100).await?;
        Ok(entries.into_iter().rev().map(|e| e.entry.kind()).collect())
    }
}
