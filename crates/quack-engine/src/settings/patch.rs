//! Updating settings (§13.1.2): facts gathered before any lock, then one
//! transaction that locks the row, checks the version when role lists
//! change, validates every sent field, writes only those fields, moves or
//! binds appeal posts, logs, and notifies the config cache.

use std::collections::BTreeSet;

use quack_core::DomainError;
use quack_core::ids::{ChannelId, GuildId, RoleId};
use quack_core::limits::{DM_FOOTER, DM_INTRO};
use quack_core::modlog::{FieldsDetail, ModLogEntry, ModLogLinks};
use quack_core::permissions::{Capability, Permission, Permissions};
use quack_core::settings::{check_role_list, normalize_invite, normalize_optional_text};
use quack_db::settings::{self, GuildSettings};
use quack_db::{Tx, messages, notify};
use serde::Serialize;

use super::{FieldProblem, SettingsError, SettingsView, view};
use crate::channels::{ChannelCheck, Gathered, check, gather};
use crate::modlog::{self, By};
use crate::opkey::{Keyed, begin_keyed, complete, request_hash};
use crate::port::Outcome;
use crate::{Engine, OpKey, Principal, obs};

/// The fields one update writes (§14.3 `SettingsPatch`): `None` leaves a
/// field alone; `Some(None)` clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SettingsPatch {
    /// Required when a role list is sent: the version the editor saw.
    pub expected_version: Option<u64>,
    /// Moderator roles (`manage_staff_roles` to change them).
    pub moderator_role_ids: Option<Vec<RoleId>>,
    /// Rules manager roles.
    pub rules_manager_role_ids: Option<Vec<RoleId>>,
    /// The log channel.
    pub log_channel_id: Option<Option<ChannelId>>,
    /// The appeal channel.
    pub appeal_channel_id: Option<Option<ChannelId>>,
    /// The evidence channel.
    pub evidence_channel_id: Option<Option<ChannelId>>,
    /// The rejoin invite (empty clears it).
    pub appeal_invite_url: Option<String>,
    /// The DM introduction (empty clears it).
    pub dm_intro: Option<String>,
    /// The DM footer (empty clears it).
    pub dm_footer: Option<String>,
}

impl SettingsPatch {
    fn field_names(&self) -> Vec<String> {
        [
            ("moderator_role_ids", self.moderator_role_ids.is_some()),
            (
                "rules_manager_role_ids",
                self.rules_manager_role_ids.is_some(),
            ),
            ("log_channel_id", self.log_channel_id.is_some()),
            ("appeal_channel_id", self.appeal_channel_id.is_some()),
            ("evidence_channel_id", self.evidence_channel_id.is_some()),
            ("appeal_invite_url", self.appeal_invite_url.is_some()),
            ("dm_intro", self.dm_intro.is_some()),
            ("dm_footer", self.dm_footer.is_some()),
        ]
        .into_iter()
        .filter(|(_, sent)| *sent)
        .map(|(name, _)| name.to_owned())
        .collect()
    }

    fn channels(&self) -> Vec<(&'static str, ChannelId, Permissions)> {
        let send = Permission::ViewChannel | Permission::SendMessages;
        [
            ("log_channel_id", self.log_channel_id.flatten(), send),
            ("appeal_channel_id", self.appeal_channel_id.flatten(), send),
            (
                "evidence_channel_id",
                self.evidence_channel_id.flatten(),
                send | Permission::AttachFiles,
            ),
        ]
        .into_iter()
        .filter_map(|(name, channel, needs)| channel.map(|c| (name, c, needs)))
        .collect()
    }
}

/// Writes the sent fields in one transaction (§13.1.2). Failures are
/// reported per field and nothing is saved.
pub async fn patch(
    engine: &Engine,
    principal: &Principal,
    patch: &SettingsPatch,
    op_key: &OpKey,
) -> Result<SettingsView, SettingsError> {
    let guild = principal.guild;
    obs::use_case("patch_settings", Some(guild), async {
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        drop(conn);
        // 1. Facts, before any lock.
        let server_roles = match engine.discord().guild_facts(guild).await {
            Outcome::Done(facts) => facts.role_ids(),
            _ => return Err(SettingsError::DiscordUnavailable),
        };
        let mut channels = Vec::new();
        for (name, channel, needs) in patch.channels() {
            let gathered = gather(engine, guild, channel).await;
            if matches!(gathered, Gathered::Unavailable) {
                return Err(SettingsError::DiscordUnavailable);
            }
            channels.push((name, gathered, needs));
        }
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let mut tx = match begin_keyed(engine.db(), &key, &request_hash(patch)).await? {
            Keyed::Replay(_) => {
                let mut conn = engine.db().conn().await?;
                return view(&mut conn, guild).await;
            }
            Keyed::Fresh(tx) => tx,
        };
        let caps = principal
            .require(&mut tx, Capability::ManageSettings)
            .await??;
        // 2–3. The row, and its version when role lists change.
        let locked = settings::lock(&mut tx, guild)
            .await?
            .ok_or(quack_db::DbError::NotFound)?;
        let roles_change =
            patch.moderator_role_ids.is_some() || patch.rules_manager_role_ids.is_some();
        if roles_change && patch.expected_version != Some(locked.version) {
            return Err(DomainError::SettingsChanged.into());
        }
        if let Some(new) = &patch.moderator_role_ids
            && as_set(new) != as_set(&locked.moderator_role_ids)
        {
            caps.check(Capability::ManageStaffRoles)
                .map_err(DomainError::from)?;
        }
        // 4. Validate every sent field.
        let db_patch = validate(guild, patch, &locked, &server_roles, &channels)?;
        // 5. Write, move or bind appeal posts, log, notify.
        settings::update(&mut tx, guild, &db_patch, locked.version, principal.account)
            .await?
            .ok_or(DomainError::SettingsChanged)?;
        appeal_posts(&mut tx, guild, &locked, patch).await?;
        modlog::write(
            &mut tx,
            guild,
            ModLogEntry::SettingsChanged(FieldsDetail {
                fields: patch.field_names(),
            }),
            By {
                actor: Some(principal.account),
                source: principal.origin.mod_log_source(),
            },
            ModLogLinks::default(),
        )
        .await?;
        notify::settings_changed(&mut tx, guild).await?;
        complete(&mut tx, &key, 200, serde_json::json!({ "guild_id": guild })).await?;
        tx.commit().await?;
        let mut conn = engine.db().conn().await?;
        view(&mut conn, guild).await
    })
    .await
}

fn as_set(roles: &[RoleId]) -> BTreeSet<RoleId> {
    roles.iter().copied().collect()
}

/// Step 4 (pure over the locked row and the gathered facts): channel checks
/// use the moderator roles being saved when the request has them.
fn validate(
    guild: GuildId,
    patch: &SettingsPatch,
    locked: &GuildSettings,
    server_roles: &[RoleId],
    channels: &[(&'static str, Gathered, Permissions)],
) -> Result<settings::SettingsPatch, SettingsError> {
    let mut problems = Vec::new();
    let mut out = settings::SettingsPatch::default();
    for (name, list, saved, slot) in [
        (
            "moderator_role_ids",
            &patch.moderator_role_ids,
            &locked.moderator_role_ids,
            &mut out.moderator_role_ids,
        ),
        (
            "rules_manager_role_ids",
            &patch.rules_manager_role_ids,
            &locked.rules_manager_role_ids,
            &mut out.rules_manager_role_ids,
        ),
    ] {
        if let Some(list) = list {
            match check_role_list(guild, list, server_roles, saved) {
                Ok(()) => *slot = Some(list.clone()),
                Err(p) => problems.push((name, FieldProblem::Value(p))),
            }
        }
    }
    if let Some(url) = &patch.appeal_invite_url {
        match normalize_invite(url) {
            Ok(v) => out.appeal_invite_url = Some(v),
            Err(p) => problems.push(("appeal_invite_url", FieldProblem::Value(p))),
        }
    }
    for (name, text, limit, slot) in [
        ("dm_intro", &patch.dm_intro, DM_INTRO, &mut out.dm_intro),
        ("dm_footer", &patch.dm_footer, DM_FOOTER, &mut out.dm_footer),
    ] {
        if let Some(text) = text {
            match normalize_optional_text(text, limit) {
                Ok(v) => *slot = Some(v),
                Err(p) => problems.push((name, FieldProblem::Value(p))),
            }
        }
    }
    let moderators = patch
        .moderator_role_ids
        .as_ref()
        .unwrap_or(&locked.moderator_role_ids);
    for (name, gathered, needs) in channels {
        match check(gathered, moderators, *needs) {
            ChannelCheck::Ok => {}
            ChannelCheck::Problems(p) => problems.push((name, FieldProblem::Channel(p))),
            ChannelCheck::Missing => problems.push((
                name,
                FieldProblem::Channel(vec![quack_core::permissions::ChannelProblem::Missing]),
            )),
            ChannelCheck::Unavailable => return Err(SettingsError::DiscordUnavailable),
        }
    }
    if !problems.is_empty() {
        return Err(SettingsError::Invalid(problems));
    }
    out.log_channel = patch.log_channel_id;
    out.appeal_channel = patch.appeal_channel_id;
    out.evidence_channel = patch.evidence_channel_id;
    Ok(out)
}

/// §8.3: a new appeal channel binds the posts of open appeals waiting for
/// one; a changed one moves the open appeals' posts there.
async fn appeal_posts(
    tx: &mut Tx,
    guild: GuildId,
    locked: &GuildSettings,
    patch: &SettingsPatch,
) -> Result<(), quack_db::DbError> {
    let Some(Some(new)) = patch.appeal_channel_id else {
        return Ok(());
    };
    if locked.appeal_channel == Some(new) {
        return Ok(());
    }
    if locked.appeal_channel.is_some() {
        messages::move_open_appeal_posts(tx, guild, new).await?;
    }
    messages::bind_waiting_appeal_posts(tx, guild, new).await?;
    Ok(())
}

/// Acknowledges the starter notice (the first time is kept).
pub async fn acknowledge_starter_notice(
    engine: &Engine,
    principal: &Principal,
    op_key: &OpKey,
) -> Result<SettingsView, SettingsError> {
    let guild = principal.guild;
    obs::use_case("acknowledge_starter_notice", Some(guild), async {
        {
            // The capability comes before the key: a replay is never served
            // to someone who lost it.
            let mut conn = engine.db().conn().await?;
            principal
                .require(&mut conn, Capability::ManageSettings)
                .await??;
        }
        let key = op_key.scoped(Some(guild), Some(principal.account));
        let hash = request_hash(&"acknowledge_starter_notice");
        if let Keyed::Fresh(mut tx) = begin_keyed(engine.db(), &key, &hash).await? {
            principal
                .require(&mut tx, Capability::ManageSettings)
                .await??;
            settings::lock(&mut tx, guild).await?;
            if settings::acknowledge_starter_notice(&mut tx, guild, principal.account).await? {
                notify::settings_changed(&mut tx, guild).await?;
            }
            complete(&mut tx, &key, 200, serde_json::json!({ "guild_id": guild })).await?;
            tx.commit().await?;
        }
        let mut conn = engine.db().conn().await?;
        principal
            .require(&mut conn, Capability::ManageSettings)
            .await??;
        view(&mut conn, guild).await
    })
    .await
}
