//! The permission matrix (§2.1, §2.4): every staff operation against every
//! kind of principal. Operations name records that don't exist, so an
//! allowed principal gets past authorization to "not found" while a
//! refused one is stopped by the capability or the 2FA rule first.

use anyhow::Result;
use quack_core::DomainError;
use quack_core::appeal::AppealDecision;
use quack_core::ids::{ActionId, AppealId, RuleId};
use quack_core::permissions::{Capability, Permission, Permissions, RoleFacts};
use quack_engine::actions::{ActionError, dismiss, retry, undo};
use quack_engine::appeals::{AppealError, decide};
use quack_engine::cases::{CaseError, add_evidence, get, void_case};
use quack_engine::rules::{RuleError, archive, create, export, list as list_rules};
use quack_engine::settings::{SettingsError, SettingsPatch, get as get_settings, health, patch};
use quack_engine::stats::{StatsError, compute};
use quack_engine::{CaseRef, Origin, Principal};
use quack_testkit::fake::MemberSetup;
use quack_testkit::rules_for_tests::warning;
use quack_testkit::{ADMIN, Harness, MEMBER, MODERATOR, OWNER, sf};

/// A refusal of the capability or the 2FA rule, if that is what happened.
fn refusal(error: Option<DomainError>) -> Option<DomainError> {
    error.filter(|e| {
        matches!(
            e,
            DomainError::MissingCapability(_) | DomainError::MfaRequired
        )
    })
}

fn of_case<T>(r: Result<T, CaseError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        CaseError::Refused(d) => Some(d),
        _ => None,
    }))
}
fn of_action<T>(r: Result<T, ActionError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        ActionError::Refused(d) => Some(d),
        _ => None,
    }))
}
fn of_appeal<T>(r: Result<T, AppealError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        AppealError::Refused(d) => Some(d),
        _ => None,
    }))
}
fn of_rule<T>(r: Result<T, RuleError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        RuleError::Refused(d) => Some(d),
        _ => None,
    }))
}
fn of_settings<T>(r: Result<T, SettingsError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        SettingsError::Refused(d) => Some(d),
        _ => None,
    }))
}
fn of_stats<T>(r: Result<T, StatsError>) -> Option<DomainError> {
    refusal(r.err().and_then(|e| match e {
        StatsError::Refused(d) => Some(d),
        _ => None,
    }))
}

type Outcomes = Vec<(&'static str, Capability, Option<DomainError>)>;

/// What each operation needs (§2.4), and how the principal fared.
async fn outcomes(h: &Harness, p: &Principal) -> Result<Outcomes> {
    let mut out = moderation(h, p).await;
    out.extend(administration(h, p).await);
    // Rules may be listed by moderators or rules managers.
    let listed = of_rule(list_rules(&h.engine, p, false).await);
    out.push(("list rules", Capability::Moderate, listed));
    Ok(out)
}

async fn moderation(h: &Harness, p: &Principal) -> Outcomes {
    let missing_case = CaseRef::Number(999);
    let action = ActionId::from_uuid(uuid::Uuid::now_v7());
    let appeal = AppealId::from_uuid(uuid::Uuid::now_v7());
    let link = vec![format!("https://discord.com/channels/{}/1/2", h.guild)];
    vec![
        (
            "view case",
            Capability::Moderate,
            of_case(get(&h.engine, p, missing_case).await),
        ),
        (
            "void case",
            Capability::Moderate,
            of_case(void_case(&h.engine, p, missing_case, "r", &h.key("vdm")).await),
        ),
        (
            "add evidence",
            Capability::Moderate,
            of_case(add_evidence(&h.engine, p, missing_case, &link, &h.key("eam")).await),
        ),
        (
            "retry",
            Capability::Moderate,
            of_action(retry(&h.engine, p, action, &h.key("ar")).await),
        ),
        (
            "dismiss",
            Capability::Moderate,
            of_action(dismiss(&h.engine, p, action, &h.key("ad")).await),
        ),
        (
            "undo",
            Capability::Moderate,
            of_action(undo(&h.engine, p, action, &h.key("auc")).await),
        ),
        (
            "decide appeal",
            Capability::Moderate,
            of_appeal(
                decide(
                    &h.engine,
                    p,
                    appeal,
                    AppealDecision::Reject,
                    "r",
                    1,
                    &h.key("ajm"),
                )
                .await,
            ),
        ),
        (
            "statistics",
            Capability::Moderate,
            of_stats(compute(&h.engine, p, None, None).await),
        ),
    ]
}

async fn administration(h: &Harness, p: &Principal) -> Outcomes {
    let rule = RuleId::from_uuid(uuid::Uuid::now_v7());
    vec![
        (
            "create rule",
            Capability::ManageRules,
            of_rule(
                create(
                    &h.engine,
                    p,
                    warning(&format!("R{}", p.account)),
                    &h.key("POST rules"),
                )
                .await,
            ),
        ),
        (
            "archive rule",
            Capability::ManageRules,
            of_rule(archive(&h.engine, p, rule, &h.key("arch")).await),
        ),
        (
            "export rule",
            Capability::ManageRules,
            of_rule(export(&h.engine, p, rule).await),
        ),
        (
            "read settings",
            Capability::ManageSettings,
            of_settings(get_settings(&h.engine, p).await),
        ),
        (
            "server health",
            Capability::ManageSettings,
            of_settings(health(&h.engine, p).await),
        ),
        (
            "change settings",
            Capability::ManageSettings,
            of_settings(
                patch(
                    &h.engine,
                    p,
                    &SettingsPatch {
                        dm_footer: Some("Thanks.".to_owned()),
                        ..SettingsPatch::default()
                    },
                    &h.key("PATCH settings"),
                )
                .await,
            ),
        ),
    ]
}

#[tokio::test]
async fn every_operation_against_every_principal() -> Result<()> {
    let h = Harness::new().await?;
    let moderator_role = sf::<quack_core::ids::RoleId>(9001);
    let manager_role = sf::<quack_core::ids::RoleId>(9002);
    h.fake.world(|w| {
        for id in [moderator_role, manager_role] {
            w.add_role(
                h.guild,
                RoleFacts {
                    id,
                    name: format!("role {id}"),
                    position: 20,
                    permissions: Permissions::NONE,
                    managed: false,
                },
            );
        }
        w.add_member(
            h.guild,
            &MemberSetup::new(sf(60), 20).roles(vec![moderator_role]),
        );
        w.add_member(
            h.guild,
            &MemberSetup::new(sf(61), 20).roles(vec![manager_role]),
        );
        w.add_member(
            h.guild,
            &MemberSetup::new(sf(62), 20).permissions(Permission::ManageGuild),
        );
    });
    h.db.execute(&format!(
        "UPDATE guild_settings SET rules_manager_role_ids = '{{{manager_role}}}' WHERE guild_id = {}",
        h.guild
    ))
    .await?;
    // (who, account, holds moderate, manage_rules, manage_settings)
    let kinds = [
        ("owner", OWNER, true, true, true),
        ("administrator", ADMIN, true, true, true),
        ("moderate members", MODERATOR, true, false, false),
        ("manage server", 62, false, true, true),
        ("rules manager role", 61, false, true, false),
        ("plain member", MEMBER, false, false, false),
    ];
    for (who, account, moderate, rules, settings) in kinds {
        let p = h.principal(account, Origin::Dashboard);
        for (op, needs, refused) in outcomes(&h, &p).await? {
            let holds = match (op, needs) {
                ("list rules", _) => moderate || rules,
                (_, Capability::Moderate) => moderate,
                (_, Capability::ManageRules) => rules,
                _ => settings,
            };
            assert_eq!(refused.is_none(), holds, "{who}: {op} ({refused:?})");
        }
    }
    // Configured moderator roles replace the Moderate Members fallback.
    h.db.execute(&format!(
        "UPDATE guild_settings SET moderator_role_ids = '{{{moderator_role}}}' WHERE guild_id = {}",
        h.guild
    ))
    .await?;
    let role_holder = h.principal(60, Origin::Dashboard);
    let fallback = h.principal(MODERATOR, Origin::Dashboard);
    for (op, needs, refused) in outcomes(&h, &role_holder).await? {
        let holds = needs == Capability::Moderate;
        assert_eq!(
            refused.is_none(),
            holds,
            "moderator role: {op} ({refused:?})"
        );
    }
    for (op, needs, refused) in outcomes(&h, &fallback).await? {
        if needs == Capability::Moderate {
            assert_eq!(
                refused,
                Some(DomainError::MissingCapability(Capability::Moderate)),
                "no fallback once a moderator role exists: {op}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_2fa_rule_blocks_every_staff_capability() -> Result<()> {
    let h = Harness::new().await?;
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&h.guild) {
            g.facts.mfa_required = true;
        }
    });
    for account in [OWNER, ADMIN, MODERATOR] {
        let without = h.principal_mfa(account, Origin::Dashboard, false);
        for (op, _, refused) in outcomes(&h, &without).await? {
            let expected = if account == MODERATOR
                && !matches!(
                    op,
                    "view case"
                        | "void case"
                        | "add evidence"
                        | "retry"
                        | "dismiss"
                        | "undo"
                        | "decide appeal"
                        | "statistics"
                        | "list rules"
                ) {
                Some(DomainError::MissingCapability(match op {
                    "create rule" | "archive rule" | "export rule" => Capability::ManageRules,
                    _ => Capability::ManageSettings,
                }))
            } else {
                Some(DomainError::MfaRequired)
            };
            assert_eq!(refused, expected, "{account} without 2FA: {op}");
        }
        let with = h.principal_mfa(account, Origin::Dashboard, true);
        let allowed = outcomes(&h, &with)
            .await?
            .into_iter()
            .filter(|(_, _, r)| *r == Some(DomainError::MfaRequired))
            .count();
        assert_eq!(allowed, 0, "{account} with 2FA");
    }
    Ok(())
}
