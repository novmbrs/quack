//! The honeypot apply path (§4.2 step 8, §13.4): Quack applies the
//! configured rule under the configuration the hits were admitted under.

use anyhow::Result;
use quack_core::DomainError;
use quack_core::case::CaseSource;
use quack_core::ids::RuleId;
use quack_core::modlog::ModLogKind;
use quack_core::permissions::{MemberRefusal, Permission};
use quack_db::modlog::{self, ModLogFilter};
use quack_engine::OpKey;
use quack_engine::cases::{Actor, Applied, ApplyError, ApplyRule, HoneypotAdmission, apply_rule};
use quack_testkit::rules_for_tests::{TIMEOUT, one};
use quack_testkit::{Harness, MEMBER, MEMBER_2, sf};

/// Turns the honeypot on for `rule` in a new channel; returns the
/// admission the module would stamp on a hit now.
async fn enable(h: &Harness, rule: RuleId) -> Result<HoneypotAdmission> {
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    h.db.execute(&format!(
        "UPDATE honeypot_settings SET enabled = true, channel_id = {channel}, rule_id = '{}',
                version = version + 1 WHERE guild_id = {}",
        rule.get(),
        h.guild
    ))
    .await?;
    let version =
        h.db.count(&format!(
            "SELECT version FROM honeypot_settings WHERE guild_id = {}",
            h.guild
        ))
        .await?;
    Ok(HoneypotAdmission {
        settings_version: u64::try_from(version)?,
        channel,
    })
}

fn request(h: &Harness, member: u64, rule: RuleId, admission: HoneypotAdmission) -> ApplyRule {
    let mut request = h.request(member, rule);
    request.actor = Actor::Quack;
    request.op_key = OpKey::new(
        "honeypot",
        format!("{member}:{}", admission.settings_version),
    );
    request.honeypot = Some(admission);
    request
}

async fn log_kinds(h: &Harness) -> Result<Vec<ModLogKind>> {
    let mut conn = h.db.db.conn().await?;
    let entries = modlog::list(&mut conn, h.guild, &ModLogFilter::default(), None, 100).await?;
    Ok(entries
        .rows
        .into_iter()
        .rev()
        .map(|e| e.entry.kind())
        .collect())
}

#[tokio::test]
async fn quack_applies_the_configured_rule() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Honeypot", TIMEOUT, false)).await?;
    let admission = enable(&h, rule).await?;
    let Applied::Created(created) =
        apply_rule(&h.engine, request(&h, MEMBER, rule, admission)).await?
    else {
        anyhow::bail!("expected a new case");
    };
    assert_eq!(created.case.source, CaseSource::Honeypot);
    assert_eq!(created.case.moderator, None);
    assert!(created.action.is_some());
    h.settle().await?;
    let end = h
        .fake
        .world(|w| w.member(h.guild, sf(MEMBER)).and_then(|m| m.timeout_until));
    assert!(end.is_some(), "the timeout ran");
    // The same hit batch again replays.
    let again = apply_rule(&h.engine, request(&h, MEMBER, rule, admission)).await?;
    assert_eq!(again, Applied::Replayed(created.case.id));
    Ok(())
}

#[tokio::test]
async fn a_changed_configuration_refuses_the_case() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Honeypot", TIMEOUT, false)).await?;
    let admitted = enable(&h, rule).await?;
    // Staff change the channel after the hits were admitted.
    let _now = enable(&h, rule).await?;
    let refused = apply_rule(&h.engine, request(&h, MEMBER, rule, admitted)).await;
    assert!(
        matches!(refused, Err(ApplyError::HoneypotPolicyChanged)),
        "{refused:?}"
    );
    // Turned off: the same.
    let current = enable(&h, rule).await?;
    h.db.execute(&format!(
        "UPDATE honeypot_settings SET enabled = false, version = version + 1 WHERE guild_id = {}",
        h.guild
    ))
    .await?;
    let refused = apply_rule(&h.engine, request(&h, MEMBER, rule, current)).await;
    assert!(
        matches!(refused, Err(ApplyError::HoneypotPolicyChanged)),
        "{refused:?}"
    );
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_member_refusal_is_left_to_the_module() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Honeypot", TIMEOUT, false)).await?;
    let admission = enable(&h, rule).await?;
    // Quack lacks Moderate Members.
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&h.guild)
            && let Some(q) = g.facts.quack.as_mut()
        {
            q.permissions = Permission::ViewChannel.into();
        }
    });
    let refused = apply_rule(&h.engine, request(&h, MEMBER_2, rule, admission)).await;
    assert!(
        matches!(
            refused,
            Err(ApplyError::Refused(DomainError::MemberRefused(
                MemberRefusal::QuackMissingPermission(_)
            )))
        ),
        "{refused:?}"
    );
    // No `case_create_refused`: the module writes `honeypot_case_failed`.
    assert_eq!(log_kinds(&h).await?, Vec::<ModLogKind>::new());
    Ok(())
}
