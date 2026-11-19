//! `guilds` → `guilds` + `case_counters`; `guild_settings` →
//! `guild_settings`; `discord_user_mfa` → `discord_accounts` (§19.4).

use std::collections::BTreeSet;

use quack_core::limits;
use quack_db::Db;
use sqlx::Row as _;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::{Ctx, GuildKey};
use crate::report::{Fragment, Report};
use crate::source::{Cols, Source};
use crate::target::{exec, strict};

/// Every server in the source, in ID order. A server whose Discord ID is
/// invalid is a problem (§19.3) and is left out.
pub(crate) async fn list(
    src: &mut Source,
    report: &mut Report,
) -> Result<Vec<GuildKey>, ImportError> {
    let rows = src
        .rows("SELECT id, discord_guild_id FROM guilds ORDER BY id", &[])
        .await?;
    let mut out = Vec::new();
    for row in &rows {
        let c = Cols::new(row, "guilds", "id");
        match c.snowflake("discord_guild_id")? {
            Snowflake::Id(id) => out.push(GuildKey {
                ulid: c.text("id")?,
                id,
            }),
            _ => report.problem(
                &format!("guild:{}", c.key()),
                "the server's Discord ID is not a snowflake; its rows are not imported".to_owned(),
            ),
        }
    }
    Ok(out)
}

/// The server row and its case counter (raised after the cases).
pub(crate) async fn import_guild(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let rows = cx
        .src
        .rows("SELECT * FROM guilds WHERE id = ?", &[&cx.g.ulid])
        .await?;
    let row = rows
        .first()
        .ok_or_else(|| ImportError::server("the server row disappeared"))?;
    let c = Cols::new(row, "guilds", "id");
    let key = cx.key("guild");
    let (mut name, changed) = convert::clean(c.text("name")?.trim().to_owned());
    if changed {
        cx.f.normalized("guilds.name", &key);
    }
    if name.is_empty() {
        "Unknown server".clone_into(&mut name);
        cx.f.derived("empty_server_name", &key);
    }
    if convert::chars(&name) > 100 {
        // Discord caps names at 100; the column's CHECK does too.
        cx.f.over_limit("guilds.name", &key);
        name = convert::truncate(&name, 100);
    }
    let icon = c.nonempty("icon_url")?;
    let icon_hash = icon.as_deref().and_then(convert::icon_hash);
    if icon.is_some() && icon_hash.is_none() {
        cx.f.derived("icon_not_a_cdn_url", &key);
    }
    let Snowflake::Id(owner) = c.snowflake("owner_discord_user_id")? else {
        return Err(ImportError::server(
            "the server owner's ID is not a snowflake",
        ));
    };
    let q = sqlx::query(
        "INSERT INTO guilds (guild_id, name, icon_hash, owner_id, mfa_required, installed, first_joined_at, left_at, updated_at)
         VALUES ($1, $2, $3, $4, false, $5, $6, NULL, $7)",
    )
    .bind(cx.g.id)
    .bind(name)
    .bind(icon_hash)
    .bind(owner)
    .bind(c.flag("is_active")?)
    .bind(c.time("created_at")?)
    .bind(c.time("updated_at")?);
    strict(&mut cx.tx, &mut cx.f, "guilds", &key, q).await?;
    let q = sqlx::query("INSERT INTO case_counters (guild_id, last_number) VALUES ($1, 0)")
        .bind(cx.g.id);
    strict(&mut cx.tx, &mut cx.f, "case_counters", &key, q).await
}

/// Parses a CSV role list: deduplicated, invalid entries reported, more
/// than 25 refused.
pub(crate) fn role_list(raw: &str) -> Result<(Vec<i64>, Vec<String>), String> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    let mut invalid = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match convert::snowflake(Some(part)) {
            Snowflake::Id(id) => {
                if seen.insert(id) {
                    ids.push(id);
                }
            }
            _ => invalid.push(part.to_owned()),
        }
    }
    if ids.len() > limits::MAX_STAFF_ROLES {
        return Err(format!(
            "{} staff roles is more than {}",
            ids.len(),
            limits::MAX_STAFF_ROLES
        ));
    }
    Ok((ids, invalid))
}

/// `guild_settings`, or the defaults when the server has no row.
#[expect(
    clippy::too_many_lines,
    reason = "one settings row's columns, read top to bottom"
)]
pub(crate) async fn import_settings(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let rows = cx
        .src
        .rows(
            "SELECT * FROM guild_settings WHERE guild_id = ? ORDER BY id",
            &[&cx.g.ulid],
        )
        .await?;
    let key = cx.key("settings");
    let Some(row) = rows.first() else {
        cx.f.derived("default_guild_settings", &key);
        let q = sqlx::query("INSERT INTO guild_settings (guild_id) VALUES ($1)").bind(cx.g.id);
        return strict(&mut cx.tx, &mut cx.f, "guild_settings", &key, q).await;
    };
    for extra in rows.iter().skip(1) {
        let c = Cols::new(extra, "guild_settings", "id");
        cx.f.skip("guild_settings", "duplicate_settings", c.key());
    }
    let c = Cols::new(row, "guild_settings", "id");
    let shape = &cx.src.shape;
    let mut roles = [Vec::new(), Vec::new()];
    for (i, column) in ["moderator_role_ids", "rules_manager_role_ids"]
        .into_iter()
        .enumerate()
    {
        if !shape.has("guild_settings", column) {
            continue;
        }
        let (ids, invalid) = role_list(&c.text(column)?)
            .map_err(|e| ImportError::server(format!("guild_settings.{column}: {e}")))?;
        for bad in invalid {
            cx.f.skip(
                "guild_settings",
                "invalid_snowflake",
                format!("{key}:{column}:{}", bad.len()),
            );
        }
        roles[i] = ids;
    }
    let channel = |cx: &mut Ctx<'_>, column: &str| -> Result<Option<i64>, ImportError> {
        match c.snowflake(column)? {
            Snowflake::Id(id) => Ok(Some(id)),
            Snowflake::None => Ok(None),
            Snowflake::Invalid => {
                cx.f.skip(
                    "guild_settings",
                    "invalid_snowflake",
                    format!("{key}:{column}"),
                );
                Ok(None)
            }
        }
    };
    let log = channel(cx, "audit_mirror_channel_discord_id")?;
    let appeal = channel(cx, "appeal_queue_channel_discord_id")?;
    let evidence = channel(cx, "managed_evidence_channel_discord_id")?;
    cx.appeal_channel = appeal;
    let invite = match c.nonempty("appeal_rejoin_url")? {
        None => None,
        Some(raw) => {
            let url = convert::invite_url(&raw);
            match &url {
                None => cx.f.skip("guild_settings", "invalid_invite", &key),
                Some(u) if *u != raw.trim() => {
                    cx.f.normalized("guild_settings.appeal_invite_url", &key);
                }
                Some(_) => {}
            }
            url
        }
    };
    let mut texts = Vec::new();
    for (column, target, limit) in [
        (
            "notification_introduction",
            "guild_settings.dm_intro",
            limits::DM_INTRO,
        ),
        (
            "notification_footer",
            "guild_settings.dm_footer",
            limits::DM_FOOTER,
        ),
    ] {
        let text = c.nonempty(column)?.map(|t| {
            let (t, changed) = convert::clean(t);
            if changed {
                cx.f.normalized(target, &key);
            }
            t
        });
        if text.as_deref().is_some_and(|t| !limit.allows(t)) {
            cx.f.over_limit(target, &key);
        }
        texts.push(text);
    }
    let starter = match c.uuid("starter_policy_template_id")? {
        Some(rule) if cx.rules.contains(&rule) => Some(rule),
        Some(_) => {
            cx.f.skip(
                "guild_settings",
                "missing_parent",
                format!("{key}:starter_rule_id"),
            );
            None
        }
        None => None,
    };
    let q = sqlx::query(
        "INSERT INTO guild_settings (guild_id, version, moderator_role_ids, rules_manager_role_ids, log_channel_id,
                                     appeal_channel_id, evidence_channel_id, appeal_invite_url, dm_intro, dm_footer,
                                     starter_rule_id, starter_notice_acknowledged_at, updated_at, updated_by)
         VALUES ($1, 1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, NULL)",
    )
    .bind(cx.g.id)
    .bind(&roles[0])
    .bind(&roles[1])
    .bind(log)
    .bind(appeal)
    .bind(evidence)
    .bind(invite)
    .bind(texts[0].take())
    .bind(texts[1].take())
    .bind(starter)
    .bind(c.opt_time("starter_policy_notice_acknowledged_at")?)
    .bind(c.time("updated_at")?);
    strict(&mut cx.tx, &mut cx.f, "guild_settings", &key, q).await
}

/// `discord_user_mfa` → `discord_accounts`, in its own transaction (rolled
/// back in a dry run). A row already there must be identical (§19.11).
pub(crate) async fn import_accounts(
    db: &Db,
    src: &mut Source,
    report: &mut Report,
    commit: bool,
) -> Result<(), ImportError> {
    if !src.shape.has_table("discord_user_mfa") {
        return Ok(());
    }
    let mut f = Fragment::default();
    let mut tx = db.begin().await?;
    exec(&mut tx, "SET LOCAL statement_timeout = '5min'").await?;
    let mut after = String::new();
    loop {
        let rows = src
            .page(
                "SELECT * FROM discord_user_mfa WHERE discord_user_id > ? ORDER BY discord_user_id",
                &[],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = last
            .try_get("discord_user_id")
            .map_err(ImportError::source)?;
        for row in &rows {
            let c = Cols::new(row, "discord_user_mfa", "discord_user_id");
            let Snowflake::Id(account) = c.snowflake("discord_user_id")? else {
                f.skip("discord_user_mfa", "invalid_snowflake", c.key());
                continue;
            };
            let (enabled, checked, updated) = (
                c.flag("mfa_enabled")?,
                c.time("mfa_checked_at")?,
                c.time("updated_at")?,
            );
            let existing = sqlx::query(
                "SELECT mfa_enabled, mfa_checked_at, updated_at FROM discord_accounts WHERE account_id = $1",
            )
            .bind(account)
            .fetch_optional(&mut *tx)
            .await
            .map_err(ImportError::target)?;
            if let Some(e) = existing {
                let same = e.try_get::<bool, _>("mfa_enabled").ok() == Some(enabled)
                    && e.try_get::<time::OffsetDateTime, _>("mfa_checked_at").ok() == Some(checked)
                    && e.try_get::<time::OffsetDateTime, _>("updated_at").ok() == Some(updated);
                if !same {
                    return Err(ImportError::Refused {
                        table: "discord_accounts",
                        key: account.to_string(),
                        detail: "a different row already exists".to_owned(),
                    });
                }
                continue;
            }
            let q = sqlx::query(
                "INSERT INTO discord_accounts (account_id, mfa_enabled, mfa_checked_at, updated_at) VALUES ($1, $2, $3, $4)",
            )
            .bind(account)
            .bind(enabled)
            .bind(checked)
            .bind(updated);
            strict(&mut tx, &mut f, "discord_accounts", &account.to_string(), q).await?;
        }
        if rows.len() < crate::source::BATCH {
            break;
        }
    }
    if commit {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    report.merge("global", f);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_lists() {
        assert_eq!(role_list("").unwrap(), (vec![], vec![]));
        assert_eq!(
            role_list("1, 2,1,,x").unwrap(),
            (vec![1, 2], vec!["x".to_owned()])
        );
        let many: Vec<String> = (1..=26).map(|i| i.to_string()).collect();
        assert!(role_list(&many.join(",")).is_err());
        let ok: Vec<String> = (1..=25).map(|i| i.to_string()).collect();
        assert_eq!(role_list(&ok.join(",")).unwrap().0.len(), 25);
    }
}
