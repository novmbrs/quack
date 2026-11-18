//! Gateway events for the core (§11.9, §17.5): server bootstrap and
//! leaving, settings repair when a channel is deleted, and hand-off of
//! every event a module wants to that module.
//!
//! The gateway's dispatcher calls this off the shard loop, one server's
//! events in order, so nothing here can block the read loop. The member
//! and guild caches are already updated by the shard loop.

use quack_core::ids::GuildId;
use quack_discord::gateway::Event;
use quack_discord::model::Guild;
use quack_engine::guilds::{bootstrap, channel_deleted, left};

use crate::bot::Shared;

/// The `guilds` row facts a server event carries.
fn facts(guild: &Guild) -> quack_db::guilds::GuildFacts {
    quack_db::guilds::GuildFacts {
        id: guild.id,
        name: guild.name.clone(),
        icon_hash: guild.icon_hash.clone(),
        owner_id: guild.owner_id,
        mfa_required: guild.mfa_required,
    }
}

/// Handles one non-interaction event.
pub(crate) async fn handle(shared: &Shared, event: &Event) {
    let engine = &shared.engine;
    let result = match event {
        Event::Ready { guild_ids, .. } => ready(shared, guild_ids).await,
        Event::GuildCreate {
            guild, channel_ids, ..
        } => guild_create(shared, guild, channel_ids).await,
        Event::GuildUpdate { guild } => bootstrap(engine, &facts(guild), None).await.map(|_| ()),
        Event::GuildDelete {
            guild_id,
            unavailable: false,
        } => left(engine, *guild_id).await,
        Event::ChannelDelete(channel) => match channel.guild_id {
            Some(guild) => channel_deleted(engine, guild, channel.id).await,
            None => Ok(()),
        },
        _ => Ok(()),
    };
    if let Err(error) = result {
        tracing::warn!(event = event.name(), %error, "couldn't handle a gateway event");
    }
    for module in shared.modules.iter().filter(|m| m.wants(event)) {
        module.on_event(engine, event).await;
    }
}

/// `READY`: installed servers missing from its list are treated as left;
/// one server failing doesn't stop the others.
async fn ready(shared: &Shared, present: &[GuildId]) -> Result<(), quack_db::DbError> {
    let engine = &shared.engine;
    let installed = {
        let mut conn = engine.db().conn().await?;
        quack_db::guilds::installed(&mut conn).await?
    };
    let mut failed = None;
    for guild in installed.into_iter().filter(|g| !present.contains(g)) {
        if let Err(error) = left(engine, guild).await {
            tracing::warn!(guild_id = guild.get(), %error, "couldn't mark a server left");
            failed = Some(error);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// `GUILD_CREATE`: bootstrap the server, reconcile its channel settings,
/// and warm its config cache (bootstrap reloads its rules).
async fn guild_create(
    shared: &Shared,
    guild: &Guild,
    channels: &[quack_core::ids::ChannelId],
) -> Result<(), quack_db::DbError> {
    let engine = &shared.engine;
    let done = bootstrap(engine, &facts(guild), Some(channels)).await?;
    if done.first_join {
        tracing::info!(guild = guild.id.get(), "joined a new server");
    }
    engine.config_cache().reload(engine.db(), guild.id).await?;
    Ok(())
}
