//! The rules cache (§11.1, §17.3): autocomplete from memory, reloads on
//! notification, and what Quack forgets when it leaves a server.

use anyhow::{Context as _, Result};
use quack_db::notify::Notification;
use quack_engine::Origin;
use quack_engine::rules::{archive, create};
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{ADMIN, Harness};

#[tokio::test]
async fn autocomplete_matches_names_and_descriptions_from_memory() -> Result<()> {
    let h = Harness::new().await?;
    let admin = h.principal(ADMIN, Origin::Dashboard);
    let mut spam = warning("Spam");
    spam.description = "Repeated links or invites.".to_owned();
    create(&h.engine, &admin, spam, &h.key("POST rules")).await?;
    let toxicity = create(&h.engine, &admin, warning("Toxicity"), &h.key("POST rules"))
        .await?
        .rule;
    let cache = h.engine.rules_cache();
    let names = |query: &str, archived: bool| -> Vec<String> {
        cache
            .autocomplete(h.guild, query, archived)
            .into_iter()
            .map(|c| c.name)
            .collect()
    };
    assert_eq!(names("", false), ["Spam", "Toxicity"]);
    assert_eq!(names("TOX", false), ["Toxicity"]);
    assert_eq!(names("invites", false), ["Spam"], "descriptions match too");
    archive(&h.engine, &admin, toxicity, &h.key("archive")).await?;
    assert_eq!(names("", false), ["Spam"]);
    assert_eq!(
        names("", true),
        ["Spam", "Toxicity"],
        "archived only when asked"
    );
    // At most 25 choices.
    for i in 0..30 {
        create(
            &h.engine,
            &admin,
            warning(&format!("Rule {i:02}")),
            &h.key("POST rules"),
        )
        .await?;
    }
    assert_eq!(cache.autocomplete(h.guild, "rule", false).len(), 25);
    Ok(())
}

#[tokio::test]
async fn a_notification_reloads_and_leaving_forgets() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(warning("Spam")).await?;
    let cache = h.engine.rules_cache();
    // Another instance renames the rule; this one hears about it.
    let mut tx = h.db.db.begin().await?;
    quack_db::rules::add_version(&mut tx, h.guild, rule, 1, &warning("Scam"), None).await?;
    tx.commit().await?;
    assert_eq!(
        cache.autocomplete(h.guild, "", false)[0].name,
        "Spam",
        "until notified"
    );
    cache
        .on_notification(&h.db.db, Notification::Rules(h.guild))
        .await?;
    assert_eq!(cache.autocomplete(h.guild, "", false)[0].name, "Scam");
    cache.forget(h.guild);
    assert!(cache.cached(h.guild).is_none());
    assert!(
        cache.autocomplete(h.guild, "", false).is_empty(),
        "memory only"
    );
    // A miss loads.
    let loaded = cache.get(&h.db.db, h.guild).await?;
    assert_eq!(loaded.active.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_config_cache_reloads_on_settings_and_never_goes_back() -> Result<()> {
    let h = Harness::new().await?;
    let cache = h.engine.config_cache();
    let first = cache.get(&h.db.db, h.guild).await?.context("installed")?;
    // Another instance changes the settings; this one hears about it.
    h.db.execute(&format!(
        "UPDATE guild_settings SET dm_footer = 'Be kind', version = version + 1 WHERE guild_id = {}",
        h.guild
    ))
    .await?;
    assert_eq!(
        cache.cached(h.guild).map(|c| c.settings.version),
        Some(first.settings.version),
        "until notified"
    );
    cache
        .on_notification(&h.db.db, Notification::Settings(h.guild))
        .await?;
    let reloaded = cache.cached(h.guild).context("cached")?;
    assert_eq!(reloaded.settings.dm_footer.as_deref(), Some("Be kind"));
    // A full reload after a reconnect keeps the newest rows.
    cache
        .on_notification(&h.db.db, Notification::Reconnected)
        .await?;
    assert_eq!(
        cache.cached(h.guild).map(|c| c.settings.version),
        Some(reloaded.settings.version)
    );
    cache.forget(h.guild);
    assert!(cache.cached(h.guild).is_none());
    Ok(())
}
