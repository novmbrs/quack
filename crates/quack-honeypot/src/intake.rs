//! From post to hit (§13.4.2 steps 1–2): a post in the honeypot channel by
//! someone who is not a bot, a webhook, or staff becomes a durable
//! `pending` hit with its coalesced `honeypot_case` job.

use quack_core::ids::{ChannelId, GuildId, MemberId, MessageId};
use quack_core::permissions::{ActorFacts, is_staff_for_access};
use quack_db::jobs::{self, NewJob};
use quack_db::{DbError, honeypot};
use quack_engine::Engine;
use quack_telemetry::labels::HoneypotOutcome;
use quack_telemetry::metrics;

use quack_engine::span;

/// The Discord dispatch events the honeypot wants: `MESSAGE_CREATE`.
/// Intents: `GUILDS`, `GUILD_MESSAGES` (the text is not needed).
pub const GATEWAY_EVENTS: &[&str] = &["MESSAGE_CREATE"];

/// A post, as the bot decodes it from `MESSAGE_CREATE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostFacts {
    /// The server.
    pub guild: GuildId,
    /// The channel it was posted in.
    pub channel: ChannelId,
    /// The post.
    pub message: MessageId,
    /// Its author.
    pub author: MemberId,
    /// The author is a bot (any bot, Quack included).
    pub author_bot: bool,
    /// A webhook posted it.
    pub webhook: bool,
    /// The author's owner flag, roles, and computed server permissions,
    /// from the message's member payload and the guild cache.
    pub actor: ActorFacts,
}

/// What became of a post.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intake {
    /// Not in an enabled honeypot channel.
    Ignored,
    /// A bot, a webhook, or staff: never a case, never deleted.
    Exempt,
    /// A new `pending` hit; its job will add the case.
    Admitted,
    /// The same post was already admitted (a repeated event).
    Duplicate,
}

/// `MESSAGE_CREATE`: admits a hit when the post is in the honeypot channel
/// of a server with the honeypot on, and its author is not exempt. Reads
/// only the config cache until it writes; the hit and its job are one
/// transaction, so a restart loses neither.
pub async fn on_message(engine: &Engine, post: &PostFacts) -> Result<Intake, DbError> {
    let Some(config) = engine.config_cache().get(engine.db(), post.guild).await? else {
        return Ok(Intake::Ignored);
    };
    let Some(settings) = config
        .honeypot
        .as_ref()
        .filter(|h| h.enabled && h.channel == Some(post.channel))
    else {
        return Ok(Intake::Ignored);
    };
    span("honeypot_hit", post.guild, async {
        if post.author_bot
            || post.webhook
            || is_staff_for_access(&post.actor, &config.settings.moderator_role_ids)
        {
            metrics().honeypot.hit(HoneypotOutcome::Exempt);
            return Ok(Intake::Exempt);
        }
        let mut tx = engine.db().begin().await?;
        let new = honeypot::insert_hit(
            &mut tx,
            post.guild,
            post.channel,
            post.message,
            post.author,
            settings.version,
            settings.rule,
        )
        .await?;
        if new {
            jobs::enqueue(&mut tx, &NewJob::honeypot_case(post.guild, post.author)).await?;
        }
        tx.commit().await?;
        tracing::info!(
            message_id = post.message.get(),
            member_id = post.author.get(),
            new,
            "honeypot hit"
        );
        Ok(if new {
            Intake::Admitted
        } else {
            Intake::Duplicate
        })
    })
    .await
}

/// Members with pending hits at most per startup pass.
const RECOVER_BATCH: u32 = 1_000;

/// Startup: makes sure every member with `pending` hits has a
/// `honeypot_case` job (enqueue coalesces with one that is already there),
/// so no hit waits forever. Returns how many members it looked at.
pub async fn recover(engine: &Engine) -> Result<u64, DbError> {
    let mut conn = engine.db().conn().await?;
    let members = honeypot::members_with_pending_hits(&mut conn, RECOVER_BATCH).await?;
    drop(conn);
    if members.is_empty() {
        return Ok(0);
    }
    let mut tx = engine.db().begin().await?;
    let jobs: Vec<NewJob> = members
        .iter()
        .map(|(guild, member)| NewJob::honeypot_case(*guild, *member))
        .collect();
    jobs::enqueue_many(&mut tx, &jobs).await?;
    tx.commit().await?;
    Ok(u64::try_from(members.len()).unwrap_or(u64::MAX))
}
