//! The honeypot module's adapter (§13.4): a post in the honeypot channel
//! becomes a hit. Its settings live in `/setup` (`handlers::setup_modules`).

use std::sync::Arc;

use quack_core::modules::ModuleName;
use quack_core::permissions::{ActorFacts, Permissions};
use quack_discord::custom_id::CustomId;
use quack_discord::gateway::Event;
use quack_discord::model::{Member, Message};
use quack_discord::permissions::member_facts;
use quack_engine::port::Outcome;
use quack_engine::{BoxFuture, Engine};
use quack_honeypot::PostFacts;
use quack_text::Msg;
use time::OffsetDateTime;

use super::{ComponentCall, Module};
use crate::ack::Failure;

struct Honeypot;

/// The honeypot module.
pub fn honeypot() -> Arc<dyn Module> {
    Arc::new(Honeypot)
}

/// Whether `m` was posted in the honeypot channel of a server with the
/// honeypot on, from the config cache alone.
fn in_honeypot(engine: &Engine, m: &Message) -> bool {
    let Some(guild) = m.guild_id else {
        return false;
    };
    engine.config_cache().cached(guild).is_some_and(|c| {
        c.honeypot
            .as_ref()
            .is_some_and(|h| h.enabled && h.channel == Some(m.channel_id))
    })
}

/// The poster's owner flag, roles, and server permissions (§13.4.2 step
/// 1): roles from the message's member payload, the rest from the
/// server's facts. Without the facts only the roles are known; the case
/// job checks the poster again from current facts.
async fn actor(engine: &Engine, m: &Message, guild: quack_core::ids::GuildId) -> ActorFacts {
    let role_ids = m.member_role_ids.clone().unwrap_or_default();
    let Outcome::Done(facts) = engine.discord().guild_facts(guild).await else {
        return ActorFacts {
            is_owner: false,
            permissions: Permissions::NONE,
            role_ids,
            mfa_enabled: false,
        };
    };
    let member = Member {
        id: m.author.id.member(),
        bot: m.author.bot,
        username: m.author.username.clone(),
        global_name: m.author.global_name.clone(),
        avatar_hash: m.author.avatar_hash.clone(),
        nick: None,
        role_ids: role_ids.clone(),
        joined_at: None,
        timeout_until: None,
    };
    let computed = member_facts(&facts, &member, OffsetDateTime::now_utc());
    ActorFacts {
        is_owner: facts.owner_id == m.author.id,
        permissions: computed.permissions,
        role_ids,
        mfa_enabled: false,
    }
}

impl Honeypot {
    async fn event(engine: &Engine, event: &Event) {
        let Event::MessageCreate(m) = event else {
            return;
        };
        let Some(guild) = m.guild_id else { return };
        if !in_honeypot(engine, m) {
            return;
        }
        let post = PostFacts {
            guild,
            channel: m.channel_id,
            message: m.id,
            author: m.author.id.member(),
            author_bot: m.author.bot,
            webhook: m.webhook,
            actor: actor(engine, m, guild).await,
        };
        if let Err(error) = quack_honeypot::on_message(engine, &post).await {
            tracing::warn!(%error, message_id = m.id.get(), "couldn't record a honeypot hit");
        }
    }
}

impl Module for Honeypot {
    fn name(&self) -> ModuleName {
        ModuleName::Honeypot
    }

    fn owns(&self, _id: &CustomId) -> bool {
        false
    }

    fn wants(&self, event: &Event) -> bool {
        matches!(event, Event::MessageCreate(m) if m.guild_id.is_some())
    }

    fn on_component<'a>(
        &'a self,
        _engine: &'a Engine,
        _call: ComponentCall<'a>,
    ) -> BoxFuture<'a, Result<Msg, Failure>> {
        Box::pin(async { Err(crate::router::expired()) })
    }

    fn on_event<'a>(&'a self, engine: &'a Engine, event: &'a Event) -> BoxFuture<'a, ()> {
        Box::pin(Self::event(engine, event))
    }
}
