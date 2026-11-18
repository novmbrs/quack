//! Honeypot settings, turning it on and off, rule protection, the default
//! rule, automatic turn-off, the warning, and cross-server isolation.

mod common;

use anyhow::{Result, bail};
use common::{admin, configured, world};
use quack_core::DomainError;
use quack_core::ids::RuleId;
use quack_core::modules::{ModuleDisabledReason, ModuleProblem};
use quack_core::permissions::{ChannelProblem, Permission};
use quack_core::rule::{Field, FieldKey, FieldKind};
use quack_core::settings::SettingsProblem;
use quack_engine::rules::{RuleError, archive};
use quack_engine::settings::{FieldProblem, SettingsError, set_enabled};
use quack_honeypot::{HoneypotSettingsInput, Intake};
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{MEMBER, sf};

async fn enable_result(w: &common::World) -> Result<Vec<ModuleProblem>> {
    match set_enabled(
        &w.h.engine,
        &admin(&w.h),
        &quack_honeypot::hooks(),
        true,
        &w.h.key("enable"),
    )
    .await
    {
        Ok(_) => Ok(Vec::new()),
        Err(SettingsError::ModuleNotReady(p)) => Ok(p),
        Err(e) => bail!("unexpected {e:?}"),
    }
}

#[tokio::test]
async fn turning_on_needs_a_channel_and_a_usable_rule() -> Result<()> {
    let w = configured().await?;
    w.save(None, None, None).await?;
    assert_eq!(
        enable_result(&w).await?,
        [
            ModuleProblem::HoneypotChannelMissing,
            ModuleProblem::HoneypotRuleMissing
        ]
    );
    // A rule that gained a required field some other way.
    let mut def = warning("Needs details");
    def.fields.push(Field {
        key: FieldKey("details".to_owned()),
        label: "Details".to_owned(),
        kind: FieldKind::ShortText,
        required: true,
    });
    let required = w.h.rule(def).await?;
    w.h.db
        .execute(&format!(
            "UPDATE honeypot_settings SET channel_id = {}, rule_id = '{}'",
            w.channel,
            required.get()
        ))
        .await?;
    assert_eq!(
        enable_result(&w).await?,
        [ModuleProblem::HoneypotRuleRequiredFields(required)]
    );
    w.h.fake.world(|world| {
        if let Some(c) = world
            .guilds
            .get_mut(&w.h.guild)
            .and_then(|g| g.channels.get_mut(&w.channel))
        {
            c.quack_permissions = Permission::ViewChannel.into();
        }
    });
    w.h.db
        .execute(&format!(
            "UPDATE honeypot_settings SET rule_id = '{}'",
            w.rule.get()
        ))
        .await?;
    let problems = enable_result(&w).await?;
    assert!(matches!(
        problems.as_slice(),
        [ModuleProblem::ChannelPermissions { .. }]
    ));
    Ok(())
}

#[tokio::test]
async fn every_field_is_validated() -> Result<()> {
    let w = world().await?;
    let current = quack_honeypot::settings(&w.h.engine, &admin(&w.h)).await?;
    let input = |channel, rule: Option<RuleId>, text: &str| HoneypotSettingsInput {
        expected_version: current.version,
        channel_id: channel,
        rule_id: rule,
        warning_text: Some(text.to_owned()),
    };
    let public =
        w.h.fake
            .world(|world| world.add_text_channel(w.h.guild, false));
    let result = quack_honeypot::update_settings(
        &w.h.engine,
        &admin(&w.h),
        &input(Some(public), Some(w.rule), &"x".repeat(1_501)),
        &w.h.key("PUT"),
    )
    .await;
    let Err(SettingsError::Invalid(problems)) = result else {
        bail!("expected problems, got {result:?}");
    };
    assert!(matches!(
        &problems[0],
        ("channel_id", FieldProblem::Channel(p)) if matches!(p.as_slice(), [ChannelProblem::QuackMissing(_)])
    ));
    assert_eq!(
        problems[1],
        (
            "warning_text",
            FieldProblem::Value(SettingsProblem::TooLong)
        )
    );
    let cleared = quack_honeypot::update_settings(
        &w.h.engine,
        &admin(&w.h),
        &input(Some(w.channel), None, ""),
        &w.h.key("PUT"),
    )
    .await;
    assert!(
        matches!(&cleared, Err(SettingsError::ModuleNotReady(p)) if p == &[ModuleProblem::HoneypotRuleMissing]),
        "the rule is cleared only while off: {cleared:?}"
    );
    let unknown = quack_honeypot::update_settings(
        &w.h.engine,
        &admin(&w.h),
        &input(
            Some(w.channel),
            Some(RuleId::from_uuid(uuid::Uuid::now_v7())),
            "",
        ),
        &w.h.key("PUT"),
    )
    .await;
    assert!(matches!(
        unknown,
        Err(SettingsError::Refused(DomainError::RuleNotFound))
    ));
    Ok(())
}

#[tokio::test]
async fn the_rule_in_use_cannot_be_archived_and_a_lost_rule_fails_hits() -> Result<()> {
    let w = world().await?;
    let refused = archive(&w.h.engine, &admin(&w.h), w.rule, &w.h.key("archive")).await;
    assert!(
        matches!(
            refused,
            Err(RuleError::Refused(DomainError::RuleUsedByHoneypot))
        ),
        "{refused:?}"
    );
    // Archived anyway (a direct database change): hits fail, never apply.
    w.h.db
        .execute(&format!(
            "UPDATE rules SET archived_at = now() WHERE id = '{}'",
            w.rule.get()
        ))
        .await?;
    w.h.engine
        .rules_cache()
        .reload(&w.h.db.db, w.h.guild)
        .await?;
    w.post(MEMBER).await?;
    w.h.settle().await?;
    assert_eq!(w.failures().await?, ["rule_unavailable"]);
    assert_eq!(w.cases().await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_deleted_channel_turns_the_honeypot_off() -> Result<()> {
    let w = world().await?;
    w.h.fake
        .world(|world| world.delete_channel(w.h.guild, w.channel));
    quack_engine::guilds::channel_deleted(&w.h.engine, w.h.guild, w.channel).await?;
    w.h.engine
        .config_cache()
        .reload(&w.h.db.db, w.h.guild)
        .await?;
    let s = quack_honeypot::settings(&w.h.engine, &admin(&w.h)).await?;
    assert!(!s.enabled);
    assert_eq!(
        s.disabled_reason,
        Some(ModuleDisabledReason::ChannelDeleted)
    );
    assert_eq!(s.channel_id, None);
    assert_eq!(w.post(MEMBER).await?.1, Intake::Ignored);
    Ok(())
}

#[tokio::test]
async fn setup_creates_the_default_rule_once() -> Result<()> {
    let w = configured().await?;
    w.save(Some(w.channel), None, None).await?;
    let saved = quack_honeypot::use_default_rule(&w.h.engine, &admin(&w.h)).await?;
    let rule = saved.rule_id.ok_or_else(|| anyhow::anyhow!("a rule"))?;
    let again = quack_honeypot::use_default_rule(&w.h.engine, &admin(&w.h)).await?;
    assert_eq!(again.rule_id, Some(rule), "nothing changes once set");
    assert_eq!(enable_result(&w).await?, []);
    // Another server state: an active "Honeypot" rule already exists.
    w.enable(false).await?;
    w.save(Some(w.channel), None, None).await?;
    let refused = quack_honeypot::use_default_rule(&w.h.engine, &admin(&w.h)).await;
    assert!(
        matches!(&refused, Err(SettingsError::ModuleNotReady(p)) if p == &[ModuleProblem::HoneypotRuleMissing]),
        "{refused:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_warning_counts_cases_and_says_when_it_is_off() -> Result<()> {
    let w = world().await?;
    w.post(MEMBER).await?;
    w.h.settle().await?;
    let shown = |w: &common::World| {
        w.h.fake.world(|world| {
            world
                .sent_to(w.channel)
                .last()
                .and_then(|m| m.msg.as_ref())
                .map_or_default(|m| format!("{m:?}"))
        })
    };
    assert!(
        shown(&w).contains("1 honeypot case so far"),
        "{}",
        shown(&w)
    );
    w.enable(false).await?;
    w.h.settle().await?;
    assert!(shown(&w).contains("The honeypot is off"), "{}", shown(&w));
    Ok(())
}

#[tokio::test]
async fn other_servers_are_untouched() -> Result<()> {
    let w = world().await?;
    let other = sf(2002);
    w.h.fake.world(|world| {
        world.add_guild(&quack_testkit::fake::GuildSetup {
            id: other,
            owner: sf(quack_testkit::OWNER),
            quack_position: 50,
            quack_permissions: quack_testkit::quack_permissions(),
            mfa_required: false,
        });
        world.add_member(other, &quack_testkit::fake::MemberSetup::new(sf(MEMBER), 5));
    });
    w.h.install(other).await?;
    // The same channel id in another server is not its honeypot.
    let (_, intake) = w.post_in(other, w.channel, MEMBER).await?;
    assert_eq!(intake, Intake::Ignored);
    w.post(MEMBER).await?;
    // A job for the same member in the other server finds nothing.
    w.h.db
        .execute(&format!(
            "INSERT INTO jobs (id, kind, key, guild_id, payload, run_at, max_attempts)
             VALUES (gen_random_uuid(), 'honeypot_case', 'honeypot_case:{other}:{MEMBER}', {other},
                     '{{\"member_id\": \"{MEMBER}\"}}', now(), 5)"
        ))
        .await?;
    w.h.settle().await?;
    assert_eq!(w.cases().await?, 1);
    assert_eq!(
        w.h.db
            .count(&format!(
                "SELECT count(*) FROM cases WHERE guild_id = {other}"
            ))
            .await?,
        0
    );
    Ok(())
}
