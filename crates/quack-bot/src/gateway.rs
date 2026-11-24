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
            guild,
            channel_ids,
            member_count,
            ..
        } => guild_create(shared, guild, channel_ids, *member_count).await,
        Event::MemberAdd { guild_id, .. } => members_moved(shared, *guild_id, 1).await,
        Event::MemberRemove { guild_id, .. } => members_moved(shared, *guild_id, -1).await,
        Event::GuildUpdate { guild } => bootstrap(engine, &facts(guild), None).await.map(|_| ()),
        Event::GuildDelete {
            guild_id,
            unavailable: false,
        } => leave(shared, *guild_id).await,
        Event::MessageDelete {
            guild_id: Some(_),
            channel_id,
            message_id,
        } => quack_engine::messages::message_deleted(engine, *channel_id, *message_id).await,
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

/// Marks a server left and, when it was installed, tells ops (§21.4).
async fn leave(shared: &Shared, guild: GuildId) -> Result<(), quack_db::DbError> {
    if left(&shared.engine, guild).await?
        && let Some(ops) = shared.ops.get()
    {
        ops.server_left(guild).await;
    }
    Ok(())
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
        if let Err(error) = leave(shared, guild).await {
            tracing::warn!(guild_id = guild.get(), %error, "couldn't mark a server left");
            failed = Some(error);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// A member joined (`+1`) or left (`-1`): moves the server's member count
/// for the ops metrics (§21.6).
async fn members_moved(
    shared: &Shared,
    guild: GuildId,
    delta: i32,
) -> Result<(), quack_db::DbError> {
    let mut conn = shared.engine.db().conn().await?;
    quack_db::guilds::add_members(&mut conn, guild, delta).await
}

/// `GUILD_CREATE`: bootstrap the server, keep its member count, reconcile
/// its channel settings, and warm its config cache (bootstrap reloads its
/// rules).
async fn guild_create(
    shared: &Shared,
    guild: &Guild,
    channels: &[quack_core::ids::ChannelId],
    members: Option<u64>,
) -> Result<(), quack_db::DbError> {
    let engine = &shared.engine;
    let done = bootstrap(engine, &facts(guild), Some(channels)).await?;
    if let Some(members) = members {
        // Before the install post, which reads it.
        let mut conn = engine.db().conn().await?;
        quack_db::guilds::set_member_count(&mut conn, guild.id, members).await?;
    }
    if done.first_join {
        tracing::info!(guild = guild.id.get(), "joined a new server");
    }
    if (done.first_join || done.returned)
        && let Some(ops) = shared.ops.get()
    {
        ops.server_joined(guild.id, done.returned).await;
    }
    engine.config_cache().reload(engine.db(), guild.id).await?;
    Ok(())
}
