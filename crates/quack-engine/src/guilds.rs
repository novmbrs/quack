//! A server's lifecycle (§17.5, §13.1.5): joining (bootstrap with the
//! starter rule), leaving, and repairing settings when a channel is
//! deleted.

use quack_core::ids::{ChannelId, GuildId, RuleId};
use quack_core::messages::MessageKind;
use quack_core::modlog::{
    ChannelSettingClearedDetail, Empty, ModLogEntry, ModLogLinks, ModuleDisabledDetail,
    ModuleSettingsChangedDetail, QuackJoinedDetail,
};
use quack_core::modules::{ChannelSetting, ModuleDisabledReason, ModuleName};
use quack_core::rule::starter_rule;
use quack_db::guilds::{self, GuildFacts};
use quack_db::messages::{self, Owner};
use quack_db::rules::{self, NewRule};
use quack_db::{DbError, Tx, activity_log, honeypot, notify, settings, tickets};

use crate::modlog::{self, By};
use crate::port::{Outcome, RefusalKind};
use crate::{Engine, obs};

/// What bootstrap found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bootstrapped {
    /// Quack joined this server for the first time (the starter rule was
    /// created).
    pub first_join: bool,
    /// Quack had left and is back.
    pub returned: bool,
    /// The starter rule, when created now.
    pub starter_rule: Option<RuleId>,
}

/// Bootstraps a server on `GUILD_CREATE` (§17.5): its row, every missing
/// child row, and, on a first join, the starter rule. Safe to repeat.
/// `channels` is the event's channel list: settings pointing at a channel
/// it lacks are reconciled when Discord confirms the channel is gone.
pub async fn bootstrap(
    engine: &Engine,
    facts: &GuildFacts,
    channels: Option<&[ChannelId]>,
) -> Result<Bootstrapped, DbError> {
    let guild = facts.id;
    let result = obs::use_case("bootstrap_guild", Some(guild), async {
        let mut tx = engine.db().begin().await?;
        let upserted = guilds::upsert(&mut tx, facts).await?;
        let created = guilds::ensure_children(&mut tx, guild).await?;
        let mut starter = None;
        if created {
            let id = RuleId::from_uuid(uuid::Uuid::now_v7());
            rules::create(
                &mut tx,
                &NewRule {
                    id,
                    guild,
                    definition: starter_rule(),
                    created_by: None,
                    reconstructed: false,
                },
            )
            .await?;
            settings::set_starter_rule(&mut tx, guild, id).await?;
            joined(&mut tx, guild, true, Some(id)).await?;
            notify::rules_changed(&mut tx, guild).await?;
            starter = Some(id);
        } else if upserted.returned {
            joined(&mut tx, guild, false, None).await?;
        }
        notify::settings_changed(&mut tx, guild).await?;
        tx.commit().await?;
        Ok::<_, DbError>(Bootstrapped {
            first_join: created,
            returned: upserted.returned,
            starter_rule: starter,
        })
    })
    .await?;
    engine.rules_cache().reload(engine.db(), guild).await?;
    if let Some(present) = channels {
        reconcile(engine, guild, present).await?;
    }
    Ok(result)
}

async fn joined(
    tx: &mut Tx,
    guild: GuildId,
    first_join: bool,
    starter: Option<RuleId>,
) -> Result<(), DbError> {
    modlog::write(
        tx,
        guild,
        ModLogEntry::QuackJoined(QuackJoinedDetail { first_join }),
        By::QUACK,
        ModLogLinks {
            rule_id: starter,
            ..ModLogLinks::default()
        },
    )
    .await?;
    Ok(())
}

/// Quack left the server (§17.5): it is marked left, every enabled module
/// is turned off (`quack_left`, logged but not posted), and its caches are
/// dropped. Settings and history are kept.
pub async fn left(engine: &Engine, guild: GuildId) -> Result<(), DbError> {
    obs::use_case("leave_guild", Some(guild), async {
        let mut tx = engine.db().begin().await?;
        if !guilds::mark_left(&mut tx, guild).await? {
            return Ok(());
        }
        for module in settings::disable_modules_on_leave(&mut tx, guild).await? {
            module_off(&mut tx, guild, module, ModuleDisabledReason::QuackLeft).await?;
        }
        modlog::write(
            &mut tx,
            guild,
            ModLogEntry::QuackLeft(Empty {}),
            By::QUACK,
            ModLogLinks::default(),
        )
        .await?;
        notify::settings_changed(&mut tx, guild).await?;
        tx.commit().await?;
        engine.rules_cache().forget(guild);
        engine.config_cache().forget(guild);
        Ok(())
    })
    .await
}

async fn module_off(
    tx: &mut Tx,
    guild: GuildId,
    module: ModuleName,
    reason: ModuleDisabledReason,
) -> Result<(), DbError> {
    modlog::write(
        tx,
        guild,
        ModLogEntry::ModuleDisabled(ModuleDisabledDetail {
            module,
            reason: Some(reason),
        }),
        By::QUACK,
        ModLogLinks::default(),
    )
    .await?;
    Ok(())
}

/// Settings pointing at a channel missing from `present` are cleared when
/// Discord confirms the channel is gone (404 Unknown Channel); a 403 or
/// any other answer keeps the setting (§13.1.5).
async fn reconcile(engine: &Engine, guild: GuildId, present: &[ChannelId]) -> Result<(), DbError> {
    let mut configured = Vec::new();
    {
        let mut conn = engine.db().conn().await?;
        if let Some(s) = settings::get(&mut conn, guild).await? {
            configured.extend([s.log_channel, s.appeal_channel, s.evidence_channel]);
        }
        if let Some(t) = tickets::settings(&mut conn, guild).await? {
            configured.extend([t.entry_channel, t.queue_channel]);
        }
        if let Some(h) = honeypot::settings(&mut conn, guild).await? {
            configured.push(h.channel);
        }
        if let Some(a) = activity_log::settings(&mut conn, guild, false).await? {
            configured.extend(a.routes.into_iter().map(|(_, c)| Some(c)));
        }
    }
    let mut missing: Vec<ChannelId> = configured
        .into_iter()
        .flatten()
        .filter(|c| !present.contains(c))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    for channel in missing {
        let gone = match engine.discord().channel_facts(guild, channel).await {
            Outcome::Done(None) => true,
            Outcome::Refused(r) => r.kind == RefusalKind::UnknownChannel,
            _ => false,
        };
        if gone {
            channel_deleted(engine, guild, channel).await?;
        }
    }
    Ok(())
}

/// A channel was deleted (`CHANNEL_DELETE`, or confirmed gone at
/// `GUILD_CREATE`): every setting that still points at it is cleared in
/// one transaction, each a compare-and-clear (§13.1.5).
pub async fn channel_deleted(
    engine: &Engine,
    guild: GuildId,
    channel: ChannelId,
) -> Result<(), DbError> {
    obs::use_case("channel_deleted", Some(guild), async {
        let mut tx = engine.db().begin().await?;
        settings::lock(&mut tx, guild).await?;
        for setting in [
            ChannelSetting::LogChannel,
            ChannelSetting::AppealChannel,
            ChannelSetting::EvidenceChannel,
        ] {
            if settings::clear_channel(&mut tx, guild, setting, channel).await? {
                cleared(&mut tx, guild, setting).await?;
            }
        }
        let (ticket_cleared, ticket_off) = tickets::clear_channel(&mut tx, guild, channel).await?;
        if ticket_off {
            module_off(
                &mut tx,
                guild,
                ModuleName::Tickets,
                ModuleDisabledReason::ChannelDeleted,
            )
            .await?;
        } else if ticket_cleared {
            cleared(&mut tx, guild, ChannelSetting::TicketEntryChannel).await?;
        }
        let (honeypot_cleared, honeypot_off) =
            honeypot::clear_channel(&mut tx, guild, channel).await?;
        if honeypot_cleared {
            if let Some(row) =
                messages::by_owner(&mut tx, guild, Owner::Server(MessageKind::HoneypotWarning))
                    .await?
            {
                messages::mark_gone(&mut tx, guild, row.id, row.generation).await?;
            }
            if honeypot_off {
                module_off(
                    &mut tx,
                    guild,
                    ModuleName::Honeypot,
                    ModuleDisabledReason::ChannelDeleted,
                )
                .await?;
            } else {
                cleared(&mut tx, guild, ChannelSetting::HoneypotChannel).await?;
            }
        }
        let (routes, activity_off) = activity_log::remove_channel(&mut tx, guild, channel).await?;
        if activity_off {
            module_off(
                &mut tx,
                guild,
                ModuleName::ActivityLog,
                ModuleDisabledReason::ChannelDeleted,
            )
            .await?;
        } else if routes > 0 {
            modlog::write(
                &mut tx,
                guild,
                ModLogEntry::ModuleSettingsChanged(ModuleSettingsChangedDetail {
                    module: ModuleName::ActivityLog,
                    fields: vec!["routes".to_owned()],
                }),
                By::QUACK,
                ModLogLinks::default(),
            )
            .await?;
        }
        notify::settings_changed(&mut tx, guild).await?;
        tx.commit().await
    })
    .await
}

async fn cleared(tx: &mut Tx, guild: GuildId, setting: ChannelSetting) -> Result<(), DbError> {
    modlog::write(
        tx,
        guild,
        ModLogEntry::ChannelSettingCleared(ChannelSettingClearedDetail { setting }),
        By::QUACK,
        ModLogLinks::default(),
    )
    .await?;
    Ok(())
}

/// The servers among `guilds` Quack is installed in (the dashboard's
/// server list).
pub async fn installed_among(engine: &Engine, guilds: &[GuildId]) -> Result<Vec<GuildId>, DbError> {
    let mut conn = engine.db().conn().await?;
    Ok(guilds::get_many(&mut conn, guilds)
        .await?
        .into_iter()
        .filter(|g| g.installed)
        .map(|g| g.id)
        .collect())
}

/// What staff are waiting on in a server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StaffCounts {
    /// Appeals waiting for a decision.
    pub pending_appeals: u64,
    /// Failed actions not dismissed.
    pub failed_actions: u64,
}

/// A server as the dashboard shows it to someone in it (route 6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSummary {
    /// The stored server.
    pub guild: guilds::Guild,
    /// The staff counts, for `moderate` (with the 2FA rule).
    pub counts: Option<StaffCounts>,
}

/// The principal's server and, when they hold `moderate`, its counts.
/// `None` when the server is not stored.
pub async fn summary(
    engine: &Engine,
    principal: &crate::Principal,
) -> Result<Option<ServerSummary>, DbError> {
    let guild = principal.guild;
    let mut conn = engine.db().conn().await?;
    let Some(stored) = guilds::get(&mut conn, guild).await? else {
        return Ok(None);
    };
    let moderates = principal
        .current_capabilities(&mut conn)
        .await?
        .check(quack_core::permissions::Capability::Moderate)
        .is_ok();
    let counts = if moderates {
        Some(StaffCounts {
            pending_appeals: quack_db::appeals::count_pending(&mut conn, guild).await?,
            failed_actions: quack_db::actions::count_failed(&mut conn, guild).await?,
        })
    } else {
        None
    };
    Ok(Some(ServerSummary {
        guild: stored,
        counts,
    }))
}
