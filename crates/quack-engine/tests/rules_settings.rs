//! Rules (§3), settings (§13.1), the guild lifecycle (§17.5), and drafts
//! (§11.6) — scenarios 4, 17, 18, 20.

mod common;

use anyhow::{Context as _, Result};
use common::{log_kinds, logged};
use quack_core::DomainError;
use quack_core::case::ContextInput;
use quack_core::ids::{ChannelId, RoleId};
use quack_core::modlog::ModLogKind;
use quack_core::permissions::{Capability, Permission, RoleFacts};
use quack_core::rule::{Field, FieldKey, FieldKind, RuleFile};
use quack_engine::cases::{Applied, ApplyError, apply_rule};
use quack_engine::drafts::{PageSaved, PageSubmit, save_page};
use quack_engine::rules::{RuleError, archive, create, edit, export, import, restore};
use quack_engine::settings::{FieldProblem, SettingsError, SettingsPatch, patch};
use quack_engine::{Origin, guilds};
use quack_testkit::rules_for_tests::{levels, warning};
use quack_testkit::{ADMIN, GUILD, Harness, MEMBER, MODERATOR, OWNER, sf};

fn admin(h: &Harness) -> quack_engine::Principal {
    h.principal(ADMIN, Origin::Dashboard)
}

#[tokio::test]
async fn edits_make_versions_and_cases_keep_theirs() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    let made = create(&h.engine, &p, warning("Spam"), &h.key("POST rules")).await?;
    let first = h.apply(MEMBER, made.rule).await?;
    let mut def = warning("Spam and ads");
    def.reason = "New reason.".to_owned();
    let edited = edit(&h.engine, &p, made.rule, 1, def.clone(), &h.key("PUT rule")).await?;
    assert_eq!(edited.version, 2);
    let stale = edit(&h.engine, &p, made.rule, 1, def, &h.key("PUT rule")).await;
    assert!(matches!(
        stale,
        Err(RuleError::Refused(DomainError::RuleChanged))
    ));
    let second = h.apply(MEMBER, made.rule).await?;
    assert_eq!(first.case.rule.map(|r| r.1), Some(1));
    assert_eq!(second.case.rule.map(|r| r.1), Some(2));
    // A pinned version that is no longer current is refused.
    let mut pinned = h.request(MEMBER, made.rule);
    pinned.pinned_version = Some(1);
    assert!(matches!(
        apply_rule(&h.engine, pinned).await,
        Err(ApplyError::Refused(DomainError::RuleChanged))
    ));
    Ok(())
}

#[tokio::test]
async fn a_replayed_rule_write_is_refused_after_losing_access() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    let made = create(&h.engine, &p, warning("Spam"), &h.key("POST rules")).await?;
    let key = h.key("PUT rule");
    let def = warning("Spam and ads");
    edit(&h.engine, &p, made.rule, 1, def.clone(), &key).await?;
    // The same person, the same request, but no longer an administrator.
    let mut demoted = p.clone();
    demoted.actor.permissions = quack_core::permissions::Permissions::NONE;
    let replay = edit(&h.engine, &demoted, made.rule, 1, def, &key).await;
    assert!(
        matches!(
            replay,
            Err(RuleError::Refused(DomainError::MissingCapability(
                Capability::ManageRules
            )))
        ),
        "{replay:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_archived_rule_cannot_be_applied_even_mid_flight() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    let made = create(&h.engine, &p, warning("Spam"), &h.key("POST rules")).await?;
    // The cache still says active; the case transaction reads the rule
    // under lock and refuses.
    let mut tx = h.db.db.begin().await?;
    quack_db::rules::archive(&mut tx, h.guild, made.rule, None).await?;
    tx.commit().await?;
    assert!(
        h.engine
            .rules_cache()
            .cached(h.guild)
            .context("cached")?
            .active(made.rule)
            .is_some()
    );
    let err = apply_rule(&h.engine, h.request(MEMBER, made.rule))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ApplyError::Refused(DomainError::RuleArchived)
    ));
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 0);
    // Restore brings it back; the name must still be free.
    restore(&h.engine, &p, made.rule, &h.key("restore")).await?;
    h.apply(MEMBER, made.rule).await?;
    Ok(())
}

#[tokio::test]
async fn names_limits_and_the_honeypot_rule() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    let a = create(&h.engine, &p, warning("Spam"), &h.key("POST rules")).await?;
    let taken = create(&h.engine, &p, warning(" spam "), &h.key("POST rules")).await;
    assert!(matches!(
        taken,
        Err(RuleError::Refused(DomainError::RuleNameTaken))
    ));
    let mut bad = warning("");
    bad.levels.clear();
    assert!(matches!(
        create(&h.engine, &p, bad, &h.key("POST rules")).await,
        Err(RuleError::Invalid(_))
    ));
    // The honeypot's rule can't be archived or gain a required field.
    h.db.execute(&format!(
        "UPDATE honeypot_settings SET rule_id = '{}' WHERE guild_id = {GUILD}",
        a.rule.get()
    ))
    .await?;
    assert!(matches!(
        archive(&h.engine, &p, a.rule, &h.key("archive")).await,
        Err(RuleError::Refused(DomainError::RuleUsedByHoneypot))
    ));
    let mut required = warning("Spam");
    required.fields = vec![Field {
        key: FieldKey("why".to_owned()),
        label: "Why?".to_owned(),
        kind: FieldKind::ShortText,
        required: true,
    }];
    assert!(matches!(
        edit(&h.engine, &p, a.rule, 1, required, &h.key("edit")).await,
        Err(RuleError::Refused(DomainError::RequiredFieldOnHoneypotRule))
    ));
    // Export and import round-trip without server data.
    let file: RuleFile = export(&h.engine, &p, a.rule).await?;
    let json = serde_json::to_value(&file)?;
    assert!(!json.to_string().contains(&GUILD.to_string()));
    let clash = import(&h.engine, &p, file.clone(), None, &h.key("import")).await;
    assert!(matches!(
        clash,
        Err(RuleError::Refused(DomainError::RuleNameTaken))
    ));
    import(
        &h.engine,
        &p,
        file,
        Some("Spam copy".to_owned()),
        &h.key("import"),
    )
    .await?;
    assert_eq!(logged(&h, ModLogKind::RuleImported).await?, 1);
    // A moderator can't manage rules.
    let m = h.principal(MODERATOR, Origin::Dashboard);
    assert!(matches!(
        create(&h.engine, &m, warning("Other"), &h.key("POST rules")).await,
        Err(RuleError::Refused(DomainError::MissingCapability(
            Capability::ManageRules
        )))
    ));
    Ok(())
}

#[tokio::test]
async fn the_active_rule_limit_holds() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    for i in 0..100 {
        create(
            &h.engine,
            &p,
            warning(&format!("Rule {i}")),
            &h.key("POST rules"),
        )
        .await?;
    }
    let one_more = create(&h.engine, &p, warning("Rule 100"), &h.key("POST rules")).await;
    assert!(matches!(
        one_more,
        Err(RuleError::Refused(DomainError::TooManyRules))
    ));
    Ok(())
}

fn add_role(
    h: &Harness,
    id: u64,
    permissions: impl Into<quack_core::permissions::Permissions>,
) -> RoleId {
    let role: RoleId = sf(id);
    h.fake.world(|w| {
        w.add_role(
            h.guild,
            RoleFacts {
                id: role,
                name: format!("role {id}"),
                position: 10,
                permissions: permissions.into(),
                managed: false,
            },
        );
    });
    role
}

#[tokio::test]
async fn settings_updates_are_checked_and_atomic() -> Result<()> {
    let h = Harness::new().await?;
    let owner = h.principal(OWNER, Origin::Dashboard);
    let role = add_role(&h, 7001, quack_core::permissions::Permissions::NONE);
    // Role lists need the version the editor saw.
    let stale = patch(
        &h.engine,
        &owner,
        &SettingsPatch {
            moderator_role_ids: Some(vec![role]),
            expected_version: Some(99),
            ..SettingsPatch::default()
        },
        &h.key("PATCH settings"),
    )
    .await;
    assert!(matches!(
        stale,
        Err(SettingsError::Refused(DomainError::SettingsChanged))
    ));
    // Manage Server can't change moderator roles.
    let manager = add_role(&h, 7002, Permission::ManageGuild);
    h.fake.world(|w| {
        if let Some(m) = w.member_mut(h.guild, sf(MODERATOR)) {
            m.role_ids.push(manager);
            m.permissions = m.permissions | Permission::ManageGuild;
        }
    });
    let mgr = h.principal(MODERATOR, Origin::Dashboard);
    let refused = patch(
        &h.engine,
        &mgr,
        &SettingsPatch {
            moderator_role_ids: Some(vec![role]),
            expected_version: Some(1),
            ..SettingsPatch::default()
        },
        &h.key("PATCH settings"),
    )
    .await;
    assert!(matches!(
        refused,
        Err(SettingsError::Refused(DomainError::MissingCapability(
            Capability::ManageStaffRoles
        )))
    ));
    Ok(())
}

#[tokio::test]
async fn settings_channels_are_checked_with_the_roles_being_saved() -> Result<()> {
    let h = Harness::new().await?;
    let owner = h.principal(OWNER, Origin::Dashboard);
    // A channel open to a non-staff role, and a bad invite, in one patch:
    // every problem is reported and nothing is saved.
    let open: ChannelId = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let bad = patch(
        &h.engine,
        &owner,
        &SettingsPatch {
            log_channel_id: Some(Some(open)),
            appeal_invite_url: Some("https://example.com/x".to_owned()),
            dm_intro: Some("Hello".to_owned()),
            ..SettingsPatch::default()
        },
        &h.key("PATCH settings"),
    )
    .await;
    let Err(SettingsError::Invalid(problems)) = bad else {
        anyhow::bail!("expected field problems");
    };
    let fields: Vec<&str> = problems.iter().map(|(f, _)| *f).collect();
    assert_eq!(fields, vec!["appeal_invite_url", "log_channel_id"]);
    assert!(matches!(problems[1].1, FieldProblem::Channel(_)));
    let mut conn = h.db.db.conn().await?;
    let s = quack_db::settings::get(&mut conn, h.guild)
        .await?
        .context("settings")?;
    assert_eq!((s.dm_intro, s.log_channel, s.version), (None, None, 1));
    drop(conn);
    // The channel check uses the moderator roles being saved.
    let staff_role = add_role(&h, 7003, quack_core::permissions::Permissions::NONE);
    let staffed: ChannelId = h.fake.world(|w| {
        let c = w.add_text_channel(h.guild, true);
        if let Some(ch) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&c))
        {
            ch.overwrites.push(quack_core::permissions::Overwrite {
                target: quack_core::permissions::OverwriteTarget::Role(staff_role),
                allow: Permission::ViewChannel.into(),
                deny: quack_core::permissions::Permissions::NONE,
            });
        }
        c
    });
    let without = patch(
        &h.engine,
        &owner,
        &SettingsPatch {
            log_channel_id: Some(Some(staffed)),
            ..SettingsPatch::default()
        },
        &h.key("PATCH settings"),
    )
    .await;
    assert!(matches!(without, Err(SettingsError::Invalid(_))));
    let with = patch(
        &h.engine,
        &owner,
        &SettingsPatch {
            moderator_role_ids: Some(vec![staff_role]),
            log_channel_id: Some(Some(staffed)),
            expected_version: Some(1),
            ..SettingsPatch::default()
        },
        &h.key("PATCH settings"),
    )
    .await?;
    assert_eq!(with.log_channel_id, Some(staffed));
    assert_eq!(with.version, 2);
    assert!(log_kinds(&h).await?.contains(&ModLogKind::SettingsChanged));
    Ok(())
}

#[tokio::test]
async fn bootstrap_leave_and_rejoin() -> Result<()> {
    let h = Harness::new().await?;
    let guild = sf::<quack_core::ids::GuildId>(2002);
    let facts = quack_db::guilds::GuildFacts {
        id: guild,
        name: "New".to_owned(),
        icon_hash: None,
        owner_id: sf(OWNER),
        mfa_required: false,
    };
    let first = guilds::bootstrap(&h.engine, &facts, None).await?;
    assert!(first.first_join && first.starter_rule.is_some());
    let again = guilds::bootstrap(&h.engine, &facts, None).await?;
    assert!(!again.first_join && !again.returned);
    h.db.execute("UPDATE ticket_settings SET entry_channel_id = 1, queue_channel_id = 2, enabled = true WHERE guild_id = 2002")
        .await?;
    guilds::left(&h.engine, guild).await?;
    let back = guilds::bootstrap(&h.engine, &facts, None).await?;
    assert!(back.returned && !back.first_join);
    let rules =
        h.db.count("SELECT count(*) FROM rules WHERE guild_id = 2002")
            .await?;
    assert_eq!(rules, 1, "rejoining never creates a second starter rule");
    let on =
        h.db.count("SELECT count(*) FROM ticket_settings WHERE guild_id = 2002 AND enabled")
            .await?;
    assert_eq!(on, 0, "leaving turned the module off");
    let reason = h
        .db
        .count("SELECT count(*) FROM ticket_settings WHERE guild_id = 2002 AND disabled_reason = 'quack_left'")
        .await?;
    assert_eq!(reason, 1);
    Ok(())
}

#[tokio::test]
async fn a_deleted_channel_clears_its_settings() -> Result<()> {
    let h = Harness::new().await?;
    let c = h.fake.world(|w| w.add_text_channel(h.guild, true));
    h.db.execute(&format!(
        "UPDATE guild_settings SET log_channel_id = {c}, appeal_channel_id = {c} WHERE guild_id = {GUILD}"
    ))
    .await?;
    guilds::channel_deleted(&h.engine, h.guild, c).await?;
    let mut conn = h.db.db.conn().await?;
    let s = quack_db::settings::get(&mut conn, h.guild)
        .await?
        .context("settings")?;
    assert_eq!((s.log_channel, s.appeal_channel), (None, None));
    assert_eq!(logged(&h, ModLogKind::ChannelSettingCleared).await?, 2);
    Ok(())
}

#[tokio::test]
async fn a_two_page_draft() -> Result<()> {
    let h = Harness::new().await?;
    let mut def = levels("Spam", &[None], false);
    def.fields = (0..7)
        .map(|i| Field {
            key: FieldKey(format!("f{i}")),
            label: format!("Field {i}"),
            kind: FieldKind::ShortText,
            required: true,
        })
        .collect();
    let rule = h.rule(def).await?;
    let draft = uuid::Uuid::now_v7();
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let page = |page: u8, keys: std::ops::Range<u32>| PageSubmit {
        draft,
        member: sf(MEMBER),
        rule,
        version: 1,
        page,
        values: keys
            .map(|i| (format!("f{i}"), format!("answer {i}")))
            .collect(),
        private: true,
        channel,
    };
    let p = h.moderator();
    let one = save_page(&h.engine, &p, &page(1, 0..5), None).await?;
    assert_eq!(one, PageSaved::Next { page: 2, pages: 2 });
    // A double click on page 1.
    let twice = save_page(&h.engine, &p, &page(1, 0..5), None).await?;
    assert_eq!(twice, PageSaved::AlreadySaved { page: 2 });
    // Another moderator can't touch it.
    let other = h.principal(quack_testkit::MODERATOR_2, Origin::Discord);
    let theirs = save_page(&h.engine, &other, &page(2, 5..7), None).await?;
    assert_eq!(theirs, PageSaved::Expired);
    // A page with a problem keeps the draft on that page.
    let mut empty = page(2, 5..7);
    empty.values[0].1 = String::new();
    assert!(matches!(
        save_page(&h.engine, &p, &empty, None).await?,
        PageSaved::Invalid(_)
    ));
    let done = save_page(&h.engine, &p, &page(2, 5..7), None).await?;
    let PageSaved::Applied(Applied::Created(created)) = done else {
        anyhow::bail!("expected a case");
    };
    assert_eq!(created.case.context.0.len(), 7);
    // Try again after the commit replays the case.
    let again = quack_engine::drafts::apply(&h.engine, &p, draft, None).await?;
    assert_eq!(
        again,
        PageSaved::Applied(Applied::Replayed(created.case.id))
    );
    // So does submitting the last page again, without a new draft.
    let resubmitted = save_page(&h.engine, &p, &page(2, 5..7), None).await?;
    assert_eq!(
        resubmitted,
        PageSaved::Applied(Applied::Replayed(created.case.id))
    );
    assert_eq!(h.db.count("SELECT count(*) FROM case_drafts").await?, 0);
    assert_eq!(h.db.count("SELECT count(*) FROM cases").await?, 1);
    let _ = ContextInput::Text(String::new());
    Ok(())
}

#[tokio::test]
async fn a_draft_whose_rule_changed_is_dropped() -> Result<()> {
    let h = Harness::new().await?;
    let p = admin(&h);
    let made = create(&h.engine, &p, warning("Spam"), &h.key("POST rules")).await?;
    edit(
        &h.engine,
        &p,
        made.rule,
        1,
        warning("Spam!"),
        &h.key("edit"),
    )
    .await?;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let submit = PageSubmit {
        draft: uuid::Uuid::now_v7(),
        member: sf(MEMBER),
        rule: made.rule,
        version: 1,
        page: 1,
        values: vec![("details".to_owned(), "x".to_owned())],
        private: true,
        channel,
    };
    let saved = save_page(&h.engine, &h.moderator(), &submit, None).await?;
    assert_eq!(saved, PageSaved::RuleChanged);
    assert_eq!(h.db.count("SELECT count(*) FROM case_drafts").await?, 0);
    Ok(())
}
