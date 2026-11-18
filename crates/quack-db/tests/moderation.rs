//! Compare-and-set state changes of actions, DMs, appeals, tracked
//! messages, and evidence, including races.

mod common;

use anyhow::{Context as _, Result};
use common::{account_id, case, db, db_with, guild, guild_id, member_id, rule};
use quack_core::action::{
    ActionFailure, ActionKind, ActionSkipReason, ActionState, AttemptOutcome, Failure,
};
use quack_core::appeal::{AppealEventKind, AppealState};
use quack_core::case::VoidCause;
use quack_core::dm::{DmFailure, DmState};
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{
    ActionId, AppealEventId, AppealId, AttachmentId, CaseId, ChannelId, DmId, EvidenceId, FileId,
    GuildId, MessageId, MessageRowId,
};
use quack_core::messages::MessageState;
use quack_core::rule::LevelAction;
use quack_db::actions::{self, Expected, NewAction};
use quack_db::appeals::{self, AppealEvent, Decision};
use quack_db::dms::{self, DmOwner};
use quack_db::evidence::{self, Admission, NewFile, Snapshot};
use quack_db::messages::{self, Location, NewTrackedMessage, Owner};
use quack_db::{Db, cases};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::OffsetDateTime;
use uuid::Uuid;

const BAN: Option<LevelAction> = Some(LevelAction::Ban {
    delete_message_seconds: 0,
});

fn channel(n: u64) -> ChannelId {
    ChannelId::new(n).unwrap_or_else(|| unreachable!())
}

fn attachment(n: u64) -> AttachmentId {
    AttachmentId::new(n).unwrap_or_else(|| unreachable!())
}

fn message(n: u64) -> MessageId {
    MessageId::new(n).unwrap_or_else(|| unreachable!())
}

async fn ban_action(db: &Db, g: GuildId, case_id: CaseId, member: u64) -> Result<ActionId> {
    let id = ActionId::from_uuid(Uuid::now_v7());
    let mut conn = db.conn().await?;
    actions::insert(
        &mut conn,
        &NewAction {
            id,
            guild: g,
            case: case_id,
            member: member_id(member),
            kind: ActionKind::Ban,
            undo_of: None,
            timeout_ends_at: None,
            delete_message_seconds: Some(0),
            requested_by: None,
            created_at: None,
        },
    )
    .await?;
    Ok(id)
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn action_attempts_follow_the_state_table(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db_with(options, 10).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), spam, BAN).await?;
    let action = ban_action(&db, g, c.id, 7).await?;

    // Two runs race to start: exactly one opens attempt 1.
    let mut starts = Vec::new();
    for _ in 0..2 {
        let db = db.clone();
        starts.push(tokio::spawn(async move {
            let mut tx = db.begin().await?;
            actions::lock_member(&mut tx, g, member_id(7)).await?;
            let started = actions::start(&mut tx, g, action).await?;
            tx.commit().await?;
            Ok::<_, anyhow::Error>(started)
        }));
    }
    let mut opened = Vec::new();
    for start in starts {
        opened.extend(start.await??);
    }
    assert_eq!(opened, vec![1]);

    let mut conn = db.conn().await?;
    assert_eq!(actions::open_attempt(&mut conn, action).await?, Some(1));
    assert!(
        actions::member_running(
            &mut conn,
            g,
            member_id(7),
            ActionId::from_uuid(Uuid::now_v7())
        )
        .await?
    );
    assert!(
        actions::finish_attempt(&mut conn, g, action, 1, AttemptOutcome::Unknown, None, None)
            .await?
    );
    assert!(
        actions::back_to_pending(&mut conn, g, action, 1, std::time::Duration::from_secs(5))
            .await?
    );
    assert!(
        !actions::mark_done(&mut conn, g, action, 1, false).await?,
        "only from running"
    );
    assert_eq!(
        actions::start(&mut conn, g, action).await?,
        Some(2),
        "numbers count across rounds"
    );
    assert!(
        actions::finish_attempt(
            &mut conn,
            g,
            action,
            2,
            AttemptOutcome::Refused,
            Some(403),
            Some(50013)
        )
        .await?
    );
    let failure = Failure {
        http_status: Some(403),
        discord_code: Some(50013),
        ..Failure::new(ActionFailure::DiscordRefused)
    };
    assert!(
        !actions::mark_failed(&mut conn, g, action, Expected::Running(1), &failure).await?,
        "a stale run of attempt 1 can't settle attempt 2's round"
    );
    assert!(actions::mark_failed(&mut conn, g, action, Expected::Running(2), &failure).await?);
    let failed = actions::get(&mut conn, g, action)
        .await?
        .context("action")?;
    assert_eq!(
        (failed.state, failed.failure, failed.attempts),
        (ActionState::Failed, Some(failure), 2)
    );
    assert_eq!(actions::count_failed(&mut conn, g).await?, 1);
    let listed = actions::list(&mut conn, g, ActionState::Failed, false, None, 50).await?;
    assert_eq!(
        listed
            .rows
            .iter()
            .map(|i| i.case_number)
            .collect::<Vec<_>>(),
        vec![c.number]
    );

    assert!(actions::retry(&mut conn, g, action, account_id(3)).await?);
    let retried = actions::get(&mut conn, g, action)
        .await?
        .context("action")?;
    assert_eq!(
        (retried.state, retried.attempts, retried.failure),
        (ActionState::Pending, 0, None)
    );
    assert!(
        !actions::dismiss(&mut conn, g, action, account_id(3)).await?,
        "only failed actions"
    );
    assert_eq!(actions::attempts(&mut conn, action).await?.len(), 2);
    assert_eq!(
        actions::cancel_pending_original(&mut conn, g, c.id).await?,
        Some(action)
    );
    assert_eq!(
        actions::cancel_pending_original(&mut conn, g, c.id).await?,
        None
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn the_undo_guard_finds_another_case_still_in_force(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let voided = case(&db, g, member_id(7), spam, BAN).await?;
    let applies = case(&db, g, member_id(7), spam, BAN).await?;
    let original = ban_action(&db, g, voided.id, 7).await?;
    let other = ban_action(&db, g, applies.id, 7).await?;
    let mut conn = db.conn().await?;
    for a in [original, other] {
        let n = actions::start(&mut conn, g, a).await?.context("started")?;
        actions::mark_done(&mut conn, g, a, n, false).await?;
    }
    cases::void(&mut conn, g, voided.id, VoidCause::Staff, None, "wrong").await?;
    let found = actions::guard(&mut conn, g, member_id(7), ActionKind::Ban, voided.id).await?;
    assert_eq!(found, Some((applies.id, applies.number)));
    assert!(actions::voided_ban_case(&mut conn, g, member_id(7), voided.number).await?);
    assert!(!actions::voided_ban_case(&mut conn, g, member_id(7), applies.number).await?);

    let undo = ActionId::from_uuid(Uuid::now_v7());
    actions::insert(
        &mut conn,
        &NewAction {
            id: undo,
            guild: g,
            case: voided.id,
            member: member_id(7),
            kind: ActionKind::Unban,
            undo_of: Some(original),
            timeout_ends_at: None,
            delete_message_seconds: None,
            requested_by: Some(account_id(3)),
            created_at: None,
        },
    )
    .await?;
    assert!(
        actions::mark_skipped(
            &mut conn,
            g,
            undo,
            Expected::Pending,
            ActionSkipReason::OtherCaseApplies,
            Some(applies.id)
        )
        .await?
    );
    let skipped = actions::undo_of(&mut conn, g, original)
        .await?
        .context("undo")?;
    assert_eq!(
        skipped.skip,
        Some((ActionSkipReason::OtherCaseApplies, Some(applies.id)))
    );
    assert_eq!(actions::for_case(&mut conn, g, voided.id).await?.len(), 2);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn dms_are_sent_at_most_once(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), spam, None).await?;
    let dm = DmId::from_uuid(Uuid::now_v7());
    let mut conn = db.conn().await?;
    dms::insert(&mut conn, dm, g, c.member, DmOwner::Case(c.id)).await?;
    assert!(
        !dms::mark_sent(&mut conn, g, dm, message(1)).await?,
        "not sending yet"
    );
    assert!(
        dms::start_sending(&mut conn, g, dm, channel(4))
            .await?
            .is_some()
    );
    assert!(
        dms::start_sending(&mut conn, g, dm, channel(4))
            .await?
            .is_none()
    );
    assert!(
        !dms::mark_failed(
            &mut conn,
            g,
            dm,
            DmState::Pending,
            DmFailure::DmsClosed,
            None
        )
        .await?
    );
    assert!(dms::back_to_pending(&mut conn, g, dm).await?);
    assert!(
        dms::start_sending(&mut conn, g, dm, channel(4))
            .await?
            .is_some()
    );
    assert!(dms::mark_sent(&mut conn, g, dm, message(9)).await?);
    assert!(
        !dms::mark_unknown(&mut conn, g, dm).await?,
        "sent is terminal"
    );
    let sent = dms::get(&mut conn, g, dm).await?.context("dm")?;
    assert_eq!(
        (sent.state, sent.message),
        (DmState::Sent, Some(message(9)))
    );
    assert!(sent.settled_at.is_some());
    assert_eq!(dms::cancel_pending_case_dm(&mut conn, g, c.id).await?, None);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn appeal_transitions_compare_state_and_version(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db_with(options, 8).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), spam, None).await?;
    let appeal = AppealId::from_uuid(Uuid::now_v7());
    let mut conn = db.conn().await?;
    appeals::insert(&mut conn, appeal, g, c.id, c.member, "I was joking").await?;
    let event = AppealEvent {
        id: AppealEventId::from_uuid(Uuid::now_v7()),
        appeal,
        kind: AppealEventKind::Submitted,
        actor: Some(account_id(7)),
        text: "I was joking".to_owned(),
        closed_by_void: false,
        // PostgreSQL keeps microseconds.
        created_at: OffsetDateTime::now_utc().replace_nanosecond(0)?,
    };
    appeals::insert_event(&mut conn, g, &event).await?;
    let again = appeals::insert(
        &mut conn,
        AppealId::from_uuid(Uuid::now_v7()),
        g,
        c.id,
        c.member,
        "x",
    )
    .await;
    assert_eq!(
        again.err().and_then(|e| e.conflict()),
        Some(quack_db::Conflict::AppealExists)
    );
    drop(conn);

    // Two staff decide at once on version 1: one wins.
    let mut racers = Vec::new();
    for (to, reason) in [
        (AppealState::Rejected, "no"),
        (AppealState::NeedsInformation, "why?"),
    ] {
        let db = db.clone();
        racers.push(tokio::spawn(async move {
            let mut tx = db.begin().await?;
            cases::lock(&mut tx, g, c.id).await?;
            appeals::lock_for_case(&mut tx, g, c.id).await?;
            let decision = Decision {
                by: account_id(3),
                reason,
            };
            let version = appeals::transition(
                &mut tx,
                g,
                appeal,
                AppealState::Pending,
                Some(1),
                to,
                Some(decision),
            )
            .await?;
            tx.commit().await?;
            Ok::<_, anyhow::Error>(version)
        }));
    }
    let mut versions = Vec::new();
    for racer in racers {
        versions.extend(racer.await??);
    }
    assert_eq!(versions, vec![2]);

    let mut conn = db.conn().await?;
    let decided = appeals::get(&mut conn, g, appeal)
        .await?
        .context("appeal")?;
    assert_eq!(decided.version, 2);
    assert_eq!(decided.decision.as_ref().map(|d| d.0), Some(account_id(3)));
    // Quack's close on void takes any version and keeps the staff decision.
    let state_now = decided.state;
    if state_now == AppealState::NeedsInformation {
        assert!(
            appeals::transition(
                &mut conn,
                g,
                appeal,
                state_now,
                None,
                AppealState::Closed,
                None
            )
            .await?
            .is_some()
        );
    }
    let closed = appeals::for_case(&mut conn, g, c.id)
        .await?
        .context("appeal")?;
    assert!(closed.first_decided_at.is_some());
    assert_eq!(closed.decision.map(|d| d.0), Some(account_id(3)));
    assert_eq!(appeals::events(&mut conn, g, appeal).await?, vec![event]);
    let page = appeals::list(&mut conn, g, None, None, 10).await?;
    assert_eq!((page.rows.len(), page.more), (1, false));
    assert_eq!(appeals::pending(&mut conn, g, 10).await?, Vec::new());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn tracked_messages_compare_the_generation(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), spam, None).await?;
    let row = MessageRowId::from_uuid(Uuid::now_v7());
    let mut tx = db.begin().await?;
    messages::insert(
        &mut tx,
        &NewTrackedMessage {
            id: row,
            guild: g,
            owner: Owner::Case(c.id),
            channel: Some(channel(5)),
            private: false,
            interaction_token: Some((
                "token".to_owned(),
                OffsetDateTime::now_utc() + time::Duration::minutes(14),
            )),
        },
    )
    .await?;
    assert_eq!(
        messages::bump(&mut tx, g, Owner::Case(c.id)).await?,
        Some(row)
    );
    tx.commit().await?;

    let mut conn = db.conn().await?;
    let read = messages::lock(&mut conn, g, row).await?.context("row")?;
    assert_eq!(
        (read.wanted_revision, read.posted_revision, read.generation),
        (2, 0, 1)
    );
    assert!(
        messages::start_create(&mut conn, g, row, 1, false)
            .await?
            .is_some()
    );
    // The message moves while the sync is running.
    let mut tx = db.begin().await?;
    messages::record_posted(&mut tx, g, row, 1, 2, Some(message(100))).await?;
    let attachment = quack_core::ids::AttachmentId::new(77).context("id")?;
    assert!(messages::set_attachment(&mut tx, g, row, 1, attachment).await?);
    assert!(messages::relocate(&mut tx, g, row, 1, Some(channel(6))).await?);
    tx.commit().await?;
    assert!(
        !messages::record_posted(&mut conn, g, row, 1, 3, Some(message(101))).await?,
        "old generation"
    );
    let moved = messages::get(&mut conn, g, row).await?.context("row")?;
    assert_eq!(moved.generation, 2);
    assert_eq!((moved.channel, moved.message), (Some(channel(6)), None));
    // Engine review MAJOR 10: the old message's attachment isn't the new
    // generation's.
    assert_eq!(moved.attachment_id, None);
    let old = Location {
        channel_id: channel(5),
        message_id: message(100),
    };
    assert_eq!(moved.stale, vec![old]);
    assert!(
        !messages::push_stale(&mut conn, g, row, old).await?,
        "deduplicated"
    );
    for n in 0..6 {
        messages::push_stale(
            &mut conn,
            g,
            row,
            Location {
                channel_id: channel(7),
                message_id: message(200 + n),
            },
        )
        .await?;
    }
    let capped = messages::get(&mut conn, g, row).await?.context("row")?;
    assert_eq!(capped.stale.len(), 5);
    assert_eq!(
        capped.stale.last().map(|l| l.message_id),
        Some(message(205))
    );
    assert!(
        messages::remove_stale(
            &mut conn,
            g,
            row,
            Location {
                channel_id: channel(7),
                message_id: message(205)
            }
        )
        .await?
    );
    assert!(messages::record_posted(&mut conn, g, row, 2, 3, Some(message(300))).await?);
    let found = messages::by_location(&mut conn, channel(6), message(300)).await?;
    assert_eq!(found.map(|m| m.id), Some(row));
    assert!(
        !messages::mark_gone(&mut conn, guild_id(99), row, 2).await?,
        "another server's row"
    );
    assert!(messages::mark_gone(&mut conn, g, row, 2).await?);
    assert!(
        !messages::record_posted(&mut conn, g, row, 2, 4, None).await?,
        "a gone row is not written at its generation"
    );
    let gone = messages::by_owner(&mut conn, g, Owner::Case(c.id))
        .await?
        .context("row")?;
    assert_eq!(gone.state, MessageState::Gone);
    let mut tx = db.begin().await?;
    assert_eq!(
        messages::bump(&mut tx, g, Owner::Case(c.id)).await?,
        None,
        "gone rows aren't bumped"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn evidence_is_admitted_once_and_saved_once(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let c = case(&db, g, member_id(7), spam, None).await?;
    let mut conn = db.conn().await?;
    let id = EvidenceId::from_uuid(Uuid::now_v7());
    let admit = async |conn: &mut sqlx::PgConnection| {
        evidence::admit(
            conn,
            g,
            c.id,
            EvidenceId::from_uuid(Uuid::now_v7()),
            channel(3),
            message(4),
            None,
        )
        .await
    };
    assert_eq!(
        evidence::admit(
            &mut conn,
            g,
            c.id,
            id,
            channel(3),
            message(4),
            Some(account_id(2))
        )
        .await?,
        Admission::Added(id)
    );
    assert_eq!(
        admit(&mut conn).await?,
        Admission::Already(id, EvidenceState::Saving)
    );
    assert!(evidence::mark_failed(&mut conn, g, id, EvidenceFailure::DiscordError).await?);
    assert_eq!(admit(&mut conn).await?, Admission::Recapture(id));
    assert_eq!(evidence::count_for_case(&mut conn, g, c.id).await?, 1);

    let snapshot = Snapshot {
        author: Some(account_id(7)),
        content: Some("spam spam".to_owned()),
        created_at: OffsetDateTime::now_utc(),
        edited_at: None,
        embeds: Vec::new(),
        files_not_saved: 0,
    };
    let file = |n: u8, state| NewFile {
        id: FileId::from_uuid(Uuid::now_v7()),
        position: n,
        attachment_id: AttachmentId::new(u64::from(n)),
        filename: format!("{n}.png"),
        content_type: Some("image/png".to_owned()),
        size_bytes: 10,
        original_url: "https://cdn.discordapp.com/x.png".to_owned(),
        copy_state: state,
    };
    let files = [
        file(1, FileCopyState::Pending),
        file(2, FileCopyState::TooLarge),
    ];
    assert!(evidence::save_snapshot(&mut conn, g, id, &snapshot, &files).await?);
    assert!(
        !evidence::save_snapshot(&mut conn, g, id, &snapshot, &files).await?,
        "stored once"
    );
    assert_eq!(evidence::count_files_for_case(&mut conn, g, c.id).await?, 2);
    assert!(
        !evidence::mark_saved(&mut conn, g, id).await?,
        "a file is still pending"
    );
    assert_eq!(
        evidence::pending_files(&mut conn, guild_id(99), id)
            .await?
            .len(),
        0,
        "another server's files"
    );
    let pending = evidence::pending_files(&mut conn, g, id).await?;
    let first = pending.first().context("pending file")?;
    assert_eq!(
        evidence::start_copy(&mut conn, guild_id(99), first.id).await?,
        None
    );
    let window = evidence::start_copy(&mut conn, g, first.id).await?;
    assert!(window.is_some());
    assert_eq!(
        evidence::start_copy(&mut conn, g, first.id).await?,
        window,
        "a retry inside the window keeps its start"
    );
    assert!(
        !evidence::mark_copied(
            &mut conn,
            guild_id(99),
            first.id,
            channel(8),
            message(9),
            attachment(10)
        )
        .await?
    );
    assert!(
        evidence::mark_copied(
            &mut conn,
            g,
            first.id,
            channel(8),
            message(9),
            attachment(10)
        )
        .await?
    );
    assert!(evidence::mark_saved(&mut conn, g, id).await?);
    let saved = evidence::get(&mut conn, g, id).await?.context("evidence")?;
    assert_eq!(
        (saved.state, saved.snapshot_stored()),
        (EvidenceState::Saved, true)
    );
    let (copied, source_channel, _) = evidence::file(&mut conn, g, c.id, first.id)
        .await?
        .context("file")?;
    assert_eq!(
        (copied.copy, source_channel),
        (Some((channel(8), message(9), attachment(10))), channel(3))
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn moving_appeal_posts_is_one_batch(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let mut rows = Vec::new();
    for member in [7, 8, 9] {
        let c = case(&db, g, member_id(member), spam, None).await?;
        let appeal = AppealId::from_uuid(Uuid::now_v7());
        let mut tx = db.begin().await?;
        appeals::insert(&mut tx, appeal, g, c.id, c.member, "Please").await?;
        let row = MessageRowId::from_uuid(Uuid::now_v7());
        messages::insert(
            &mut tx,
            &NewTrackedMessage {
                id: row,
                guild: g,
                owner: Owner::Appeal(appeal),
                channel: Some(channel(5)),
                private: false,
                interaction_token: None,
            },
        )
        .await?;
        tx.commit().await?;
        let mut conn = db.conn().await?;
        assert!(
            messages::record_posted(&mut conn, g, row, 1, 1, Some(message(100 + member))).await?
        );
        rows.push(row);
    }
    let mut tx = db.begin().await?;
    let mut moved = messages::move_open_appeal_posts(&mut tx, g, channel(6)).await?;
    tx.commit().await?;
    moved.sort();
    rows.sort();
    assert_eq!(moved, rows);
    let mut conn = db.conn().await?;
    for row in &rows {
        let m = messages::get(&mut conn, g, *row).await?.context("row")?;
        assert_eq!(
            (m.generation, m.channel, m.message),
            (2, Some(channel(6)), None)
        );
        assert_eq!(m.stale.len(), 1, "the old post is cleaned up later");
    }
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'sync_message'")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(jobs, 3, "one coalesced sync job per row");
    Ok(())
}
