//! Evidence review fixes (§7): copies are never sources, restores and
//! recaptures run under the current actor's access, member file URLs
//! respect a change made meanwhile, replays don't depend on the evidence
//! channel, staging has ceilings, and the copy job spends its budget.

#![allow(clippy::too_many_lines, reason = "scenarios read top to bottom")]

mod common;

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use quack_core::DomainError;
use quack_core::case::{ContextInput, LinkProblem};
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{AccountId, CaseId, ChannelId, EvidenceId, MessageId};
use quack_core::permissions::{Overwrite, OverwriteTarget, Permission, Permissions};
use quack_db::evidence::Evidence;
use quack_db::jobs::{JobKind, JobResult, NewJob};
use quack_engine::cases::{
    AddEvidence, ApplyError, CaseError, add_evidence, apply_rule, edit_context, member_case,
};
use quack_engine::evidence::{
    FileError, FileViewer, NewUpload, gallery, remove_evidence, resolve_file, set_visibility,
};
use quack_engine::port::RefusalKind;
use quack_engine::{CaseRef, Origin};
use quack_testkit::RunEnd;
use quack_testkit::fake::{Call, Gate, Script};
use quack_testkit::rules_for_tests::levels;
use quack_testkit::{ADMIN, GUILD, Harness, MEMBER, MODERATOR, MODERATOR_2, sf};

fn link(channel: ChannelId, message: MessageId) -> String {
    format!("https://discord.com/channels/{GUILD}/{channel}/{message}")
}

async fn items(h: &Harness, case: CaseId) -> Result<Vec<Evidence>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::evidence::for_case(&mut conn, h.guild, case).await?)
}

async fn item(h: &Harness, case: CaseId, id: EvidenceId) -> Result<Evidence> {
    items(h, case)
        .await?
        .into_iter()
        .find(|e| e.id == id)
        .context("item")
}

async fn set_channel(h: &Harness, channel: ChannelId) -> Result<()> {
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = {channel} WHERE guild_id = {GUILD}"
    ))
    .await?;
    Ok(())
}

/// A case, a public channel, and a staff-only evidence channel.
async fn setup(h: &Harness) -> Result<(CaseId, ChannelId, ChannelId)> {
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let evidence = h.fake.world(|w| w.add_text_channel(h.guild, true));
    set_channel(h, evidence).await?;
    Ok((case, channel, evidence))
}

/// Uploads one image from the dashboard (staff-only) and copies it;
/// returns the item and Quack's copy post.
async fn upload_copied(h: &Harness, case: CaseId) -> Result<(EvidenceId, ChannelId, MessageId)> {
    let added = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence {
            files: vec![NewUpload::bytes(
                "secret.png",
                Some("image/png".to_owned()),
                vec![1, 2, 3],
            )],
            ..AddEvidence::default()
        },
        &h.key("POST uploads"),
    )
    .await?;
    h.settle().await?;
    let id = added.upload.context("upload")?;
    let mut conn = h.db.db.conn().await?;
    let file = quack_db::evidence::files_for_case(&mut conn, h.guild, case)
        .await?
        .into_iter()
        .find(|f| f.evidence == id)
        .context("file")?;
    let (channel, message, _) = file.copy.context("copied")?;
    Ok((id, channel, message))
}

/// Review A: Quack's copy of a staff-only upload, linked as a message, is
/// refused on every path (Add evidence, case creation, context edits), in
/// the evidence channel and in a former one, and the capture job fails it
/// if it gets in anyway, so the member never sees the file.
#[tokio::test]
async fn quacks_copy_posts_are_never_evidence() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, evidence) = setup(&h).await?;
    let (upload, copy_channel, copy) = upload_copied(&h, case).await?;
    assert_eq!(copy_channel, evidence);
    // An administrator reads the evidence channel, so access is no bar.
    let admin = h.principal(ADMIN, Origin::Dashboard);
    let copy_link = link(copy_channel, copy);
    let add = |p| {
        let links = vec![copy_link.clone()];
        let h = &h;
        async move {
            add_evidence(
                &h.engine,
                &p,
                CaseRef::Id(case),
                &AddEvidence::links(&links),
                &h.key("eam"),
            )
            .await
        }
    };
    let refused = add(admin.clone()).await;
    assert!(
        matches!(&refused, Err(CaseError::LinksInvalid(p)) if p == &[(0, LinkProblem::EvidenceChannel)]),
        "{refused:?}"
    );
    // Any message in the evidence channel is refused, not only copies.
    let chat = h
        .fake
        .world(|w| w.post(h.guild, evidence, sf(ADMIN), "staff chat", vec![]));
    let refused = add_evidence(
        &h.engine,
        &admin,
        CaseRef::Id(case),
        &AddEvidence::links(&[link(evidence, chat)]),
        &h.key("eam"),
    )
    .await;
    assert!(matches!(refused, Err(CaseError::LinksInvalid(_))));
    // The evidence channel moved on: the old copy is still a copy.
    let newer = h.fake.world(|w| w.add_text_channel(h.guild, true));
    set_channel(&h, newer).await?;
    let refused = add(admin.clone()).await;
    assert!(
        matches!(refused, Err(CaseError::LinksInvalid(_))),
        "{refused:?}"
    );
    // Case creation: an explicit link, and a message link field.
    let rule = h
        .rule({
            let mut def = levels("Leak", &[None], false);
            def.fields = vec![quack_core::rule::Field {
                key: quack_core::rule::FieldKey("proof".to_owned()),
                label: "Proof".to_owned(),
                kind: quack_core::rule::FieldKind::MessageLink,
                required: false,
            }];
            def
        })
        .await?;
    let mut request = h.request(MEMBER, rule);
    request.evidence_links = vec![copy_link.clone()];
    let refused = apply_rule(&h.engine, request).await;
    assert!(
        matches!(refused, Err(ApplyError::LinksInvalid(_))),
        "{refused:?}"
    );
    let mut request = h.request(MEMBER, rule);
    request.context = vec![("proof".to_owned(), ContextInput::Text(copy_link.clone()))];
    let refused = apply_rule(&h.engine, request).await;
    assert!(
        matches!(&refused, Err(ApplyError::ContextInvalid(p)) if p[0].code == quack_core::case::ContextProblemCode::EvidenceChannel),
        "{refused:?}"
    );
    // A context edit that names it.
    let created = h.apply(MEMBER, rule).await?.case;
    let refused = edit_context(
        &h.engine,
        &admin,
        CaseRef::Id(created.id),
        &[("proof".to_owned(), ContextInput::Text(copy_link.clone()))],
        0,
        &h.key("cem"),
    )
    .await;
    assert!(
        matches!(refused, Err(CaseError::ContextInvalid(_))),
        "{refused:?}"
    );
    // Got in anyway (a race, or a path that isn't checked up front): the
    // capture fails it without saving the message or its file.
    let id = EvidenceId::from_uuid(uuid::Uuid::now_v7());
    {
        let mut tx = h.db.db.begin().await?;
        quack_db::evidence::admit(
            &mut tx,
            h.guild,
            case,
            id,
            (copy_channel, copy),
            Some(sf(ADMIN)),
            None,
        )
        .await?;
        quack_db::jobs::enqueue(&mut tx, &NewJob::capture_evidence(h.guild, id)).await?;
        tx.commit().await?;
    }
    h.settle().await?;
    let sneaked = item(&h, case, id).await?;
    assert_eq!(
        (sneaked.state, sneaked.failure),
        (EvidenceState::Failed, Some(EvidenceFailure::EvidenceCopy))
    );
    let seen = member_case(&h.engine, sf::<AccountId>(MEMBER), case).await?;
    assert!(
        seen.evidence.iter().all(|e| e.files.is_empty()),
        "the member sees no file: {:?}",
        seen.evidence
    );
    assert!(!item(&h, case, upload).await?.visible_to_member);
    Ok(())
}

/// Denies @everyone View Channel in `channel` (the moderators lose it).
fn close_channel(h: &Harness, channel: ChannelId) {
    h.fake.world(|w| {
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&channel))
        {
            c.overwrites.push(Overwrite {
                target: OverwriteTarget::Role(h.guild.everyone_role()),
                allow: Permissions::NONE,
                deny: Permission::ViewChannel.into(),
            });
        }
    });
}

/// Lets `member` read `channel` whatever @everyone may.
fn open_channel_to(h: &Harness, channel: ChannelId, member: u64) {
    h.fake.world(|w| {
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&channel))
        {
            c.overwrites.push(Overwrite {
                target: OverwriteTarget::Member(sf(member)),
                allow: Permission::ViewChannel | Permission::ReadMessageHistory,
                deny: Permissions::NONE,
            });
        }
    });
}

/// Review B: adding a link again restores or captures again only for
/// someone who can read its channel now, and the capture runs under that
/// person's access while the original adder stays recorded.
#[tokio::test]
async fn restoring_or_capturing_again_needs_the_current_actors_access() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h).await?;
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "spam", vec![]));
    let add = |account: u64| {
        let links = vec![link(channel, m)];
        let h = &h;
        async move {
            add_evidence(
                &h.engine,
                &h.principal(account, Origin::Discord),
                CaseRef::Id(case),
                &AddEvidence::links(&links),
                &h.key("eam"),
            )
            .await
        }
    };
    add(MODERATOR).await?;
    h.settle().await?;
    let id = items(&h, case).await?[0].id;
    remove_evidence(&h.engine, &h.moderator(), None, id, &h.key("evrc")).await?;
    // The moderator can't read the channel any more: no restore.
    close_channel(&h, channel);
    let refused = add(MODERATOR).await;
    assert!(
        matches!(&refused, Err(CaseError::LinksInvalid(p)) if p == &[(0, LinkProblem::NoAccess)]),
        "{refused:?}"
    );
    assert!(item(&h, case, id).await?.is_removed(), "still removed");
    // Someone who can read it restores it.
    open_channel_to(&h, channel, MODERATOR_2);
    let restored = add(MODERATOR_2).await?;
    assert_eq!(restored.restored, 1);
    assert!(!item(&h, case, id).await?.is_removed());

    // A failed item: captured again only by someone who can read it, and
    // under their access, with the first adder kept.
    let f = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "more spam", vec![]));
    h.fake.script(
        Call::FetchMessage,
        Script::refuse(RefusalKind::MissingAccess),
    );
    add_evidence(
        &h.engine,
        &h.principal(MODERATOR_2, Origin::Discord),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, f)]),
        &h.key("eam"),
    )
    .await?;
    h.settle().await?;
    let failed = items(&h, case)
        .await?
        .into_iter()
        .find(|e| e.message == Some(f))
        .context("failed item")?;
    assert_eq!(failed.state, EvidenceState::Failed);
    let refused = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, f)]),
        &h.key("eam"),
    )
    .await;
    assert!(
        matches!(refused, Err(CaseError::LinksInvalid(_))),
        "{refused:?}"
    );
    assert_eq!(
        item(&h, case, failed.id).await?.state,
        EvidenceState::Failed
    );
    open_channel_to(&h, channel, MODERATOR);
    let again = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, f)]),
        &h.key("eam"),
    )
    .await?;
    assert_eq!(again.retrying, 1);
    let saving = item(&h, case, failed.id).await?;
    assert_eq!(
        (saving.added_by, saving.capture_by),
        (Some(sf(MODERATOR_2)), Some(sf(MODERATOR))),
        "the adder is kept; the capture runs under the current actor"
    );
    // The current actor loses access before the capture runs: it fails,
    // although the first adder could still read the channel.
    h.fake.world(|w| {
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&channel))
        {
            c.overwrites
                .retain(|o| o.target != OverwriteTarget::Member(sf(MODERATOR)));
        }
    });
    h.settle().await?;
    assert_eq!(
        item(&h, case, failed.id).await?.failure,
        Some(EvidenceFailure::ModeratorNoAccess)
    );
    Ok(())
}

/// Review C: staff hide an uploaded file while the member's file route
/// waits on Discord: the member gets no URL.
#[tokio::test]
async fn a_member_gets_no_url_for_a_file_hidden_meanwhile() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h).await?;
    let (upload, _, _) = upload_copied(&h, case).await?;
    let staff = h.moderator();
    set_visibility(&h.engine, &staff, None, upload, true, &h.key("evv")).await?;
    let file = {
        let mut conn = h.db.db.conn().await?;
        quack_db::evidence::files_for_case(&mut conn, h.guild, case)
            .await?
            .into_iter()
            .find(|f| f.evidence == upload)
            .context("file")?
            .id
    };
    let gate = Gate::new();
    h.fake
        .script(Call::FetchMessage, Script::Hold(gate.clone()));
    let member = FileViewer::Member {
        account: sf(MEMBER),
        case,
    };
    let engine = h.engine.clone();
    let asked = tokio::spawn(async move { resolve_file(&engine, member, file).await });
    gate.reached().await;
    set_visibility(&h.engine, &staff, None, upload, false, &h.key("evv")).await?;
    gate.release();
    assert!(matches!(asked.await?, Err(FileError::NotFound)));
    Ok(())
}

/// Review H: an upload whose answer was lost replays once the evidence
/// channel is gone, instead of failing the channel check.
#[tokio::test]
async fn an_upload_replays_without_its_evidence_channel() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h).await?;
    let key = h.key("POST uploads");
    let add = AddEvidence {
        files: vec![NewUpload::bytes(
            "a.png",
            Some("image/png".to_owned()),
            vec![9],
        )],
        ..AddEvidence::default()
    };
    let first = add_evidence(&h.engine, &h.moderator(), CaseRef::Id(case), &add, &key).await?;
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = NULL WHERE guild_id = {GUILD}"
    ))
    .await?;
    let again = add_evidence(&h.engine, &h.moderator(), CaseRef::Id(case), &add, &key).await?;
    assert!(again.replayed);
    assert_eq!(again.upload, first.upload);
    // Another request under the key is still refused.
    let other = AddEvidence {
        files: vec![NewUpload::bytes(
            "b.png",
            Some("image/png".to_owned()),
            vec![8],
        )],
        ..AddEvidence::default()
    };
    assert!(matches!(
        add_evidence(&h.engine, &h.moderator(), CaseRef::Id(case), &other, &key).await,
        Err(CaseError::OperationKeyReused)
    ));
    Ok(())
}

/// Review G: staged bytes have a ceiling per server; past it an upload is
/// refused as busy and nothing is saved.
#[tokio::test]
async fn staged_uploads_have_a_ceiling() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h).await?;
    // 27 files of 10 MiB (three uploads of nine) waiting for their copy
    // (zeros compress, so the test database stays small).
    h.db.execute(&format!(
        "WITH e AS (
           INSERT INTO evidence (id, guild_id, case_id, kind, added_by, visible_to_member)
           SELECT gen_random_uuid(), {GUILD}, '{case}', 'upload', {MODERATOR}, false
           FROM generate_series(1, 3) RETURNING id),
         f AS (
           INSERT INTO evidence_files (id, guild_id, case_id, evidence_id, position, filename, size_bytes)
           SELECT gen_random_uuid(), {GUILD}, '{case}', e.id, i, 'big.png', 10485760
           FROM e, generate_series(1, 9) AS i RETURNING id)
         INSERT INTO evidence_upload_bytes (file_id, guild_id, bytes)
         SELECT id, {GUILD}, decode(repeat('00', 10485760), 'hex') FROM f",
        case = case.get()
    ))
    .await?;
    let add = AddEvidence {
        files: vec![NewUpload::bytes(
            "a.png",
            Some("image/png".to_owned()),
            vec![9],
        )],
        ..AddEvidence::default()
    };
    let refused = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &add,
        &h.key("up"),
    )
    .await;
    assert!(
        matches!(refused, Err(CaseError::Refused(DomainError::UploadsBusy))),
        "{refused:?}"
    );
    assert_eq!(
        h.db.count(&format!(
            "SELECT count(*) FROM evidence WHERE case_id = '{}' AND kind = 'upload'",
            case.get()
        ))
        .await?,
        3,
        "nothing saved"
    );
    // Once the copies drained, uploads go through again.
    h.db.execute("DELETE FROM evidence_upload_bytes").await?;
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &add,
        &h.key("up"),
    )
    .await?;
    Ok(())
}

/// Review I: a download that fails and leaves the run no time to go on
/// costs the job a counted attempt (a free `Continue` would start every
/// run afresh, so the file would stay pending forever).
#[tokio::test]
async fn a_failed_download_that_ends_the_run_costs_an_attempt() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h).await?;
    let attachment = h
        .fake
        .world(|w| w.attachment("a.png", Some("image/png"), vec![1]));
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![attachment]));
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, m)]),
        &h.key("eam"),
    )
    .await?;
    // The first run stores the snapshot; its download then hangs and fails
    // with just too little time left for another try.
    let gate = Gate::new();
    h.fake.script(
        Call::Download,
        Script::HoldThen(gate.clone(), Box::new(Script::Unknown)),
    );
    let mut job = h
        .claim(JobKind::CaptureEvidence)
        .await?
        .context("capture job")?;
    job.deadline = Instant::now() + Duration::from_secs(76);
    let engine = h.engine.clone();
    let run = tokio::spawn(async move { quack_testkit::run_claimed(&engine, job).await });
    gate.reached().await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    gate.release();
    let ran = run.await?;
    assert!(
        matches!(&ran.end, RunEnd::Settled(JobResult::Retry { error, .. }) if error == "download_failed"),
        "{:?}",
        ran.end
    );
    Ok(())
}

/// Review J: a not-sent upload's try is given back under the job's fence,
/// so a worker that lost its lease can't give back a newer worker's try.
#[tokio::test]
async fn a_stale_worker_cant_give_back_a_newer_workers_try() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h).await?;
    let (first, second) = (Gate::new(), Gate::new());
    h.fake.script(
        Call::Send,
        Script::HoldThen(first.clone(), Box::new(Script::NotSent)),
    );
    h.fake.script(
        Call::Send,
        Script::HoldThen(second.clone(), Box::new(Script::Pass)),
    );
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence {
            files: vec![NewUpload::bytes(
                "a.png",
                Some("image/png".to_owned()),
                vec![1],
            )],
            ..AddEvidence::default()
        },
        &h.key("up"),
    )
    .await?;
    let old = h.spawn_one(JobKind::CaptureEvidence).await?;
    first.reached().await;
    h.expire_leases().await?;
    let new = h.spawn_one(JobKind::CaptureEvidence).await?;
    second.reached().await;
    let attempts = || {
        h.db.count("SELECT sum(copy_attempts)::bigint FROM evidence_files")
    };
    assert_eq!(attempts().await?, 2, "both workers reserved a try");
    // The old worker's upload wasn't sent; its lease is gone, so it gives
    // nothing back.
    first.release();
    assert_eq!(old.await?.end, RunEnd::LostLease);
    assert_eq!(
        attempts().await?,
        2,
        "the newer worker's try stays reserved"
    );
    second.release();
    new.await?;
    let state: Vec<FileCopyState> = {
        let mut conn = h.db.db.conn().await?;
        quack_db::evidence::files_for_case(&mut conn, h.guild, case)
            .await?
            .into_iter()
            .map(|f| f.copy_state)
            .collect()
    };
    assert_eq!(state, vec![FileCopyState::Copied]);
    Ok(())
}

/// Review K: without Read Message History in the evidence channel a copy
/// could be saved but never opened, so uploads are refused up front.
#[tokio::test]
async fn uploads_need_read_message_history_in_the_evidence_channel() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, evidence) = setup(&h).await?;
    h.fake.world(|w| {
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&evidence))
        {
            c.quack_permissions =
                Permissions(c.quack_permissions.0 & !Permission::ReadMessageHistory.bit());
        }
    });
    let refused = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence {
            files: vec![NewUpload::bytes(
                "a.png",
                Some("image/png".to_owned()),
                vec![1],
            )],
            ..AddEvidence::default()
        },
        &h.key("up"),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(CaseError::Refused(DomainError::EvidenceChannelNotStaffOnly))
        ),
        "{refused:?}"
    );
    Ok(())
}

/// Review N: the gallery shows only copies; when a copy is gone the file
/// is left out (the file route still falls back to the original).
#[tokio::test]
async fn the_gallery_never_falls_back_to_the_original() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h).await?;
    let attachment = h
        .fake
        .world(|w| w.attachment("a.png", Some("image/png"), vec![1]));
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![attachment]));
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, m)]),
        &h.key("eam"),
    )
    .await?;
    h.settle().await?;
    let saved = items(&h, case).await?[0].clone();
    let file = {
        let mut conn = h.db.db.conn().await?;
        quack_db::evidence::files_for_case(&mut conn, h.guild, case).await?[0].clone()
    };
    let (_, copy, _) = file.copy.context("copied")?;
    assert_eq!(
        gallery(&h.engine, &h.moderator(), CaseRef::Id(case), saved.id)
            .await?
            .len(),
        1
    );
    h.fake.world(|w| w.messages.remove(&copy));
    assert!(
        gallery(&h.engine, &h.moderator(), CaseRef::Id(case), saved.id)
            .await?
            .is_empty(),
        "no copy, no gallery item"
    );
    let moderator = h.moderator();
    let staff = FileViewer::Staff {
        principal: &moderator,
        case: CaseRef::Id(case),
    };
    assert!(resolve_file(&h.engine, staff, file.id).await.is_ok());
    Ok(())
}
