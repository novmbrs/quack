//! Ticket settings (§13.2.1): validation, the version check, the panel
//! following the entry channel, and turning tickets off.

mod common;

use anyhow::{Result, bail};
use common::{M, admin, world};
use quack_core::DomainError;
use quack_core::permissions::ChannelProblem;
use quack_core::settings::SettingsProblem;
use quack_engine::settings::{FieldProblem, SettingsError, set_enabled};
use quack_tickets::{OpenAnswer, TicketSettingsInput};

#[tokio::test]
async fn every_field_is_validated() -> Result<()> {
    let w = world().await?;
    let current = quack_tickets::settings(&w.h.engine, &admin(&w.h)).await?;
    let public =
        w.h.fake
            .world(|world| world.add_text_channel(w.h.guild, false));
    let result = quack_tickets::update_settings(
        &w.h.engine,
        &admin(&w.h),
        &TicketSettingsInput {
            expected_version: current.version,
            entry_channel_id: Some(quack_testkit::sf(424_242)),
            queue_channel_id: Some(public),
            transcript_retention_days: 0,
        },
        &w.h.key("PUT"),
    )
    .await;
    let Err(SettingsError::Invalid(problems)) = result else {
        bail!("expected problems, got {result:?}");
    };
    let fields: Vec<_> = problems.iter().map(|(f, _)| *f).collect();
    assert_eq!(
        fields,
        [
            "entry_channel_id",
            "queue_channel_id",
            "transcript_retention_days"
        ]
    );
    assert_eq!(
        problems[0].1,
        FieldProblem::Channel(vec![ChannelProblem::Missing])
    );
    assert!(matches!(
        &problems[1].1,
        FieldProblem::Channel(p) if p.contains(&ChannelProblem::EveryoneCanView)
    ));
    assert_eq!(
        problems[2].1,
        FieldProblem::Value(SettingsProblem::RetentionDays)
    );
    let stale = quack_tickets::update_settings(
        &w.h.engine,
        &admin(&w.h),
        &TicketSettingsInput {
            expected_version: current.version - 1,
            entry_channel_id: Some(w.entry),
            queue_channel_id: Some(w.queue),
            transcript_retention_days: 30,
        },
        &w.h.key("PUT"),
    )
    .await;
    assert!(
        matches!(
            stale,
            Err(SettingsError::Refused(DomainError::SettingsChanged))
        ),
        "{stale:?}"
    );
    let moderator = quack_tickets::settings(&w.h.engine, &w.h.moderator()).await;
    assert!(matches!(moderator, Err(SettingsError::Refused(_))));
    Ok(())
}

#[tokio::test]
async fn the_panel_moves_with_the_entry_channel() -> Result<()> {
    let w = world().await?;
    let new_entry = w.h.fake.world(|world| {
        let c = world.add_text_channel(w.h.guild, false);
        if let Some(f) = world
            .guilds
            .get_mut(&w.h.guild)
            .and_then(|g| g.channels.get_mut(&c))
        {
            f.quack_permissions = common::entry_permissions();
        }
        c
    });
    let current = quack_tickets::settings(&w.h.engine, &admin(&w.h)).await?;
    let input = TicketSettingsInput {
        expected_version: current.version,
        entry_channel_id: Some(new_entry),
        queue_channel_id: Some(w.queue),
        transcript_retention_days: 60,
    };
    let key = w.h.key("PUT");
    let saved = quack_tickets::update_settings(&w.h.engine, &admin(&w.h), &input, &key).await?;
    assert_eq!(saved.entry_channel_id, Some(new_entry));
    assert_eq!(saved.version, current.version + 1);
    // The same request again replays instead of failing the version check.
    let replay = quack_tickets::update_settings(&w.h.engine, &admin(&w.h), &input, &key).await?;
    assert_eq!(replay, saved);
    w.h.settle().await?;
    let (old, new) =
        w.h.fake
            .world(|world| (world.sent_to(w.entry).len(), world.sent_to(new_entry).len()));
    assert_eq!(new, 1, "the panel is posted in the new channel");
    assert!(old <= 1, "the old panel says it moved or is gone");
    let OpenAnswer::Opening { entry, .. } = w.open(M).await? else {
        bail!("expected opening");
    };
    assert_eq!(entry, new_entry);
    Ok(())
}

#[tokio::test]
async fn turning_tickets_off_keeps_open_tickets_closable() -> Result<()> {
    let w = world().await?;
    let t = w.opened(M).await?;
    set_enabled(
        &w.h.engine,
        &admin(&w.h),
        &quack_tickets::hooks(),
        false,
        &w.h.key("disable"),
    )
    .await?;
    assert_eq!(w.open(quack_testkit::MEMBER_2).await?, OpenAnswer::Disabled);
    w.close_as(&t, M).await?;
    w.h.settle().await?;
    assert!(w.live(M).await?.is_none());
    Ok(())
}
