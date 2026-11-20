//! Uploaded files, notes, visibility, removal, surrounding messages, and
//! the "Add as evidence" picker (§7, §20.1.1 scenario 16).

mod common;

use anyhow::{Context as _, Result};
use quack_core::DomainError;
use quack_core::evidence::{
    EvidenceFailure, EvidenceKind, EvidenceState, FileCopyState, FileRefusal,
};
use quack_core::ids::{AttachmentId, CaseId, ChannelId, MessageId};
use quack_core::modlog::ModLogKind;
use quack_db::evidence::Evidence;
use quack_engine::CaseRef;
use quack_engine::cases::{AddEvidence, CaseError, add_evidence, member_case};
use quack_engine::evidence::{
    FileError, FileViewer, NewUpload, UploadSource, gallery, recent_cases, remove_evidence,
    resolve_file, set_note, set_visibility,
};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::levels;
use quack_testkit::{GUILD, Harness, MEMBER, MODERATOR, sf};

fn link(channel: ChannelId, message: MessageId) -> String {
    format!("https://discord.com/channels/{GUILD}/{channel}/{message}")
}

async fn items(h: &Harness, case: CaseId) -> Result<Vec<Evidence>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::evidence::for_case(&mut conn, h.guild, case).await?)
}

async fn files(h: &Harness, case: CaseId) -> Result<Vec<quack_db::evidence::EvidenceFile>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::evidence::files_for_case(&mut conn, h.guild, case).await?)
}

async fn kinds_logged(h: &Harness, case: CaseId) -> Result<Vec<ModLogKind>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::modlog::for_case(&mut conn, h.guild, case, 100)
        .await?
        .iter()
        .map(|e| e.kind)
        .collect())
}

/// A case, a public channel, and (when `copies`) a staff-only evidence
/// channel saved in the settings.
async fn setup(h: &Harness, copies: bool) -> Result<(CaseId, ChannelId, Option<ChannelId>)> {
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let evidence = if copies {
        let c = h.fake.world(|w| w.add_text_channel(h.guild, true));
        h.db.execute(&format!(
            "UPDATE guild_settings SET evidence_channel_id = {c} WHERE guild_id = {GUILD}"
        ))
        .await?;
        Some(c)
    } else {
        None
    };
    Ok((case, channel, evidence))
}

/// An interaction attachment, served at a short-lived CDN URL.
fn discord_file(h: &Harness, name: &str, content_type: &str, bytes: Vec<u8>) -> NewUpload {
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let (id, url) = h.fake.world(|w| {
        let id = w.next_id();
        let url = format!("https://cdn.discordapp.com/ephemeral-attachments/1/{id}/{name}");
        w.files.insert(url.clone(), bytes);
        (id, url)
    });
    NewUpload {
        filename: name.to_owned(),
        content_type: Some(content_type.to_owned()),
        size_bytes: size,
        source: UploadSource::Discord {
            attachment_id: AttachmentId::new(id).unwrap_or_else(|| unreachable!()),
            url,
        },
    }
}

fn upload(files: Vec<NewUpload>, note: Option<&str>) -> AddEvidence {
    AddEvidence {
        links: Vec::new(),
        files,
        note: note.map(str::to_owned),
    }
}

#[tokio::test]
async fn uploaded_files_are_copied_and_start_staff_only() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, copies) = setup(&h, true).await?;
    let copies = copies.context("evidence channel")?;
    let files_in = vec![
        discord_file(&h, "proof.png", "image/png", vec![1, 2, 3]),
        NewUpload::bytes("../clip.mp4", Some("video/mp4".to_owned()), vec![4, 5]),
    ];
    let added = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &upload(files_in, Some("  From a ticket.  ")),
        &h.key("eam"),
    )
    .await?;
    assert_eq!((added.files, added.new), (2, 0));
    let id = added.upload.context("upload item")?;
    h.settle().await?;
    let item = items(&h, case)
        .await?
        .into_iter()
        .find(|e| e.id == id)
        .context("item")?;
    assert_eq!(
        (
            item.kind,
            item.state,
            item.visible_to_member,
            item.note.as_deref()
        ),
        (
            EvidenceKind::Upload,
            EvidenceState::Saved,
            false,
            Some("From a ticket.")
        )
    );
    let names: Vec<(String, FileCopyState)> = files(&h, case)
        .await?
        .into_iter()
        .map(|f| (f.filename, f.copy_state))
        .collect();
    assert_eq!(
        names,
        vec![
            ("proof.png".to_owned(), FileCopyState::Copied),
            ("clip.mp4".to_owned(), FileCopyState::Copied)
        ]
    );
    let sent: Vec<String> = h.fake.world(|w| {
        w.sent_to(copies)
            .iter()
            .map(|m| m.fetched.attachments[0].filename.clone())
            .collect()
    });
    assert_eq!(sent, vec!["SPOILER_proof.png", "SPOILER_clip.mp4"]);
    let staged: i64 =
        h.db.count("SELECT count(*) FROM evidence_upload_bytes")
            .await?;
    assert_eq!(staged, 0, "staged bytes go once copied");
    assert!(
        kinds_logged(&h, case)
            .await?
            .contains(&ModLogKind::EvidenceAdded)
    );
    // The gallery serves fresh URLs of the copies.
    let shown = gallery(&h.engine, &h.moderator(), CaseRef::Id(case), id).await?;
    assert_eq!(shown.len(), 2);
    assert!(
        shown
            .iter()
            .all(|g| g.url.starts_with("https://cdn.discordapp.com/"))
    );
    Ok(())
}

#[tokio::test]
async fn uploads_are_refused_before_anything_is_saved() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h, false).await?;
    let file = || discord_file(&h, "a.png", "image/png", vec![1]);
    let add = |files| {
        let h = &h;
        async move {
            add_evidence(
                &h.engine,
                &h.moderator(),
                CaseRef::Id(case),
                &upload(files, None),
                &h.key("eam"),
            )
            .await
        }
    };
    // No evidence channel.
    assert!(matches!(
        add(vec![file()]).await,
        Err(CaseError::Refused(DomainError::EvidenceChannelMissing))
    ));
    // An evidence channel everyone can see.
    let open = h.fake.world(|w| w.add_text_channel(h.guild, false));
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = {open} WHERE guild_id = {GUILD}"
    ))
    .await?;
    assert!(matches!(
        add(vec![file()]).await,
        Err(CaseError::Refused(DomainError::EvidenceChannelNotStaffOnly))
    ));
    let staff_only = h.fake.world(|w| w.add_text_channel(h.guild, true));
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = {staff_only} WHERE guild_id = {GUILD}"
    ))
    .await?;
    // Files Quack wouldn't copy, by name; more than 10.
    let big = NewUpload {
        size_bytes: 10_485_761,
        ..discord_file(&h, "big.mp4", "video/mp4", vec![1])
    };
    let exe = discord_file(&h, "setup.exe", "application/x-msdownload", vec![1]);
    match add(vec![file(), big, exe]).await {
        Err(CaseError::FilesRefused(refused)) => {
            let names: Vec<(&str, FileRefusal)> = refused
                .iter()
                .map(|r| (r.filename.as_str(), r.why))
                .collect();
            assert_eq!(
                names,
                vec![
                    ("big.mp4", FileRefusal::TooLarge),
                    ("setup.exe", FileRefusal::UnsupportedType)
                ]
            );
        }
        other => panic!("expected FilesRefused, got {other:?}"),
    }
    assert!(matches!(
        add((0..11).map(|_| file()).collect()).await,
        Err(CaseError::Refused(DomainError::TooManyFiles))
    ));
    // Nothing at all, and a note too long.
    assert!(matches!(add(vec![]).await, Err(CaseError::NoLinks)));
    let long_note = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &upload(vec![file()], Some(&"a".repeat(501))),
        &h.key("eam"),
    )
    .await;
    assert!(matches!(long_note, Err(CaseError::NoteLength)));
    assert!(items(&h, case).await?.is_empty(), "nothing was saved");
    Ok(())
}

#[tokio::test]
async fn an_upload_with_no_copy_fails_and_a_replay_adds_nothing() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _, _) = setup(&h, true).await?;
    h.fake
        .script_n(Call::Download, 3, &Script::refuse(RefusalKind::Other));
    let request = upload(vec![discord_file(&h, "a.png", "image/png", vec![1])], None);
    let key = h.key("eam");
    let first = add_evidence(&h.engine, &h.moderator(), CaseRef::Id(case), &request, &key).await?;
    let again = add_evidence(&h.engine, &h.moderator(), CaseRef::Id(case), &request, &key).await?;
    assert!(again.replayed);
    assert_eq!(again.upload, first.upload);
    h.settle().await?;
    let saved = items(&h, case).await?;
    assert_eq!(saved.len(), 1, "one item for one request");
    assert_eq!(
        (saved[0].state, saved[0].failure),
        (EvidenceState::Failed, Some(EvidenceFailure::NoFileCopied))
    );
    Ok(())
}

#[allow(clippy::too_many_lines, reason = "one story, step by step")]
#[tokio::test]
async fn removed_items_leave_every_view_and_come_back_when_added_again() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h, true).await?;
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "spam", vec![]));
    let staff = h.moderator();
    let added = add_evidence(
        &h.engine,
        &staff,
        CaseRef::Id(case),
        &AddEvidence {
            links: vec![link(channel, m)],
            files: vec![discord_file(&h, "a.png", "image/png", vec![1])],
            note: Some("both".to_owned()),
        },
        &h.key("eam"),
    )
    .await?;
    h.settle().await?;
    let upload_id = added.upload.context("upload")?;
    let message_item = items(&h, case)
        .await?
        .into_iter()
        .find(|e| e.kind == EvidenceKind::Message)
        .context("message item")?;
    assert_eq!(
        message_item.note.as_deref(),
        Some("both"),
        "the note goes on each item"
    );
    let member = sf::<quack_core::ids::AccountId>(MEMBER);
    let seen = member_case(&h.engine, member, case).await?;
    assert_eq!(seen.evidence.len(), 1, "the upload is staff-only");
    let upload_file = files(&h, case)
        .await?
        .into_iter()
        .find(|f| f.evidence == upload_id)
        .context("file")?;
    let as_member = FileViewer::Member {
        account: member,
        case,
    };
    assert!(matches!(
        resolve_file(&h.engine, as_member, upload_file.id).await,
        Err(FileError::NotFound)
    ));
    // Shown to the member: their page and the file route have it.
    let shown = set_visibility(&h.engine, &staff, None, upload_id, true, &h.key("evv")).await?;
    assert!(shown.changed);
    let again = set_visibility(&h.engine, &staff, None, upload_id, true, &h.key("evv")).await?;
    assert!(!again.changed, "already visible");
    assert_eq!(
        member_case(&h.engine, member, case).await?.evidence.len(),
        2
    );
    assert!(
        resolve_file(&h.engine, as_member, upload_file.id)
            .await
            .is_ok()
    );
    // Saved messages have no switch.
    assert!(matches!(
        set_visibility(
            &h.engine,
            &staff,
            None,
            message_item.id,
            false,
            &h.key("evv")
        )
        .await,
        Err(CaseError::Refused(DomainError::EvidenceNotUpload))
    ));
    // Removal: hidden from the member, recorded, out of the cap.
    let removed = remove_evidence(
        &h.engine,
        &staff,
        Some(CaseRef::Id(case)),
        message_item.id,
        &h.key("evrc"),
    )
    .await?;
    assert!(removed.changed);
    remove_evidence(&h.engine, &staff, None, upload_id, &h.key("evrc")).await?;
    assert_eq!(
        member_case(&h.engine, member, case).await?.evidence.len(),
        0
    );
    assert!(matches!(
        resolve_file(&h.engine, as_member, upload_file.id).await,
        Err(FileError::NotFound)
    ));
    let logged = kinds_logged(&h, case).await?;
    assert!(logged.contains(&ModLogKind::EvidenceRemoved));
    assert!(logged.contains(&ModLogKind::EvidenceVisibilityChanged));
    assert!(matches!(
        set_note(
            &h.engine,
            &staff,
            None,
            upload_id,
            Some("x"),
            0,
            &h.key("evnm")
        )
        .await,
        Err(CaseError::Refused(DomainError::EvidenceRemoved))
    ));
    // Another case's item through this case is not found.
    let other_rule = h.rule(levels("Other", &[None], false)).await?;
    let other = h.apply(MEMBER, other_rule).await?.case.id;
    assert!(matches!(
        remove_evidence(
            &h.engine,
            &staff,
            Some(CaseRef::Id(other)),
            upload_id,
            &h.key("evrc")
        )
        .await,
        Err(CaseError::Refused(DomainError::EvidenceNotFound))
    ));
    // The same link again brings the message back.
    let back = add_evidence(
        &h.engine,
        &staff,
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, m)]),
        &h.key("eam"),
    )
    .await?;
    assert_eq!((back.restored, back.new), (1, 0));
    assert_eq!(
        member_case(&h.engine, member, case).await?.evidence.len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn notes_compare_their_revision() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h, false).await?;
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, m)]),
        &h.key("eam"),
    )
    .await?;
    let id = items(&h, case).await?[0].id;
    let staff = h.moderator();
    let first = set_note(
        &h.engine,
        &staff,
        Some(CaseRef::Id(case)),
        id,
        Some("one"),
        0,
        &h.key("evnm"),
    )
    .await?;
    assert!(first.changed);
    assert!(matches!(
        set_note(&h.engine, &staff, None, id, Some("two"), 0, &h.key("evnm")).await,
        Err(CaseError::Refused(DomainError::NoteChanged))
    ));
    let cleared = set_note(&h.engine, &staff, None, id, Some("  "), 1, &h.key("evnm")).await?;
    assert!(cleared.changed);
    let item = &items(&h, case).await?[0];
    assert_eq!((item.note.as_deref(), item.note_revision), (None, 2));
    assert!(
        !kinds_logged(&h, case)
            .await?
            .contains(&ModLogKind::EvidenceRemoved),
        "notes are not recorded"
    );
    Ok(())
}

#[tokio::test]
async fn the_messages_around_a_staff_added_message_are_saved() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel, _) = setup(&h, false).await?;
    let post = |text: &str| {
        h.fake
            .world(|w| w.post(h.guild, channel, sf(MEMBER), text, vec![]))
    };
    let before: Vec<MessageId> = (0..7).map(|i| post(&format!("before {i}"))).collect();
    let target = post("the one");
    let after: Vec<MessageId> = (0..2).map(|i| post(&format!("after {i}"))).collect();
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, target)]),
        &h.key("eam"),
    )
    .await?;
    h.settle().await?;
    let item = &items(&h, case).await?[0];
    let around = item.surrounding.as_ref().context("surrounding saved")?;
    let ids: Vec<MessageId> = around.iter().map(|m| m.message_id).collect();
    let mut expected: Vec<MessageId> = before[2..].to_vec();
    expected.extend(&after);
    assert_eq!(ids, expected, "the 5 closest before, then those after");
    assert!(around[..5].iter().all(|m| m.before));
    assert_eq!(around[5].content.as_deref(), Some("after 0"));

    // A failed read never fails the capture.
    let lone = post("lonely");
    h.fake.script(
        Call::FetchHistory,
        Script::refuse(RefusalKind::MissingAccess),
    );
    h.fake.script(
        Call::FetchHistoryAfter,
        Script::refuse(RefusalKind::MissingAccess),
    );
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &AddEvidence::links(&[link(channel, lone)]),
        &h.key("eam"),
    )
    .await?;
    h.settle().await?;
    let item = items(&h, case)
        .await?
        .into_iter()
        .find(|e| e.message == Some(lone))
        .context("item")?;
    assert_eq!((item.state, item.surrounding), (EvidenceState::Saved, None));
    Ok(())
}

#[tokio::test]
async fn a_case_takes_its_file_and_is_refused_without_a_channel() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let mut request = h.request(MEMBER, rule);
    request.evidence_files = vec![discord_file(&h, "a.png", "image/png", vec![1])];
    let refused = quack_engine::cases::apply_rule(&h.engine, request.clone()).await;
    assert!(matches!(
        refused,
        Err(quack_engine::cases::ApplyError::Refused(
            DomainError::EvidenceChannelMissing
        ))
    ));
    let cases: i64 = h.db.count("SELECT count(*) FROM cases").await?;
    assert_eq!(cases, 0, "no case without its file");
    let c = h.fake.world(|w| w.add_text_channel(h.guild, true));
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = {c} WHERE guild_id = {GUILD}"
    ))
    .await?;
    let created = match quack_engine::cases::apply_rule(&h.engine, request).await? {
        quack_engine::cases::Applied::Created(c) => c,
        other @ quack_engine::cases::Applied::Replayed(_) => panic!("{other:?}"),
    };
    h.settle().await?;
    let saved = items(&h, created.case.id).await?;
    assert_eq!(saved.len(), 1);
    assert_eq!(
        (saved[0].kind, saved[0].state, saved[0].added_by),
        (
            EvidenceKind::Upload,
            EvidenceState::Saved,
            Some(sf(MODERATOR))
        )
    );
    Ok(())
}

#[tokio::test]
async fn the_picker_offers_the_authors_and_the_actors_cases() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let first = h.apply(MEMBER, rule).await?.case;
    let second = h.apply(quack_testkit::MEMBER_2, rule).await?.case;
    let offered = recent_cases(&h.engine, &h.moderator(), Some(sf(MEMBER))).await?;
    let numbers: Vec<u64> = offered.iter().map(|c| c.number).collect();
    assert_eq!(numbers, vec![second.number, first.number]);
    Ok(())
}

#[tokio::test]
async fn a_draft_keeps_its_file_until_the_case_is_applied() -> Result<()> {
    use quack_engine::drafts::{CommandDraft, DraftFile, apply, start};
    let h = Harness::new().await?;
    let (_, channel, _) = setup(&h, true).await?;
    let rule = h.rule(levels("Harassment", &[None], false)).await?;
    let file = |name: &str, content_type: &str| {
        let NewUpload {
            source: UploadSource::Discord { attachment_id, url },
            size_bytes,
            ..
        } = discord_file(&h, name, content_type, vec![1, 2])
        else {
            unreachable!("a Discord file")
        };
        DraftFile {
            attachment_id,
            filename: name.to_owned(),
            content_type: Some(content_type.to_owned()),
            size_bytes,
            url,
        }
    };
    let draft = |evidence_file| CommandDraft {
        draft: uuid::Uuid::now_v7(),
        member: sf(MEMBER),
        rule: Some((rule, 1)),
        source: None,
        private: true,
        channel,
        evidence_file: Some(evidence_file),
    };
    let refused = start(
        &h.engine,
        &h.moderator(),
        &draft(file("tool.exe", "application/zip")),
    )
    .await;
    assert!(matches!(
        refused,
        Err(quack_engine::cases::ApplyError::FilesRefused(_))
    ));
    let form = draft(file("proof.png", "image/png"));
    start(&h.engine, &h.moderator(), &form).await?;
    let applied = apply(&h.engine, &h.moderator(), form.draft, None).await?;
    let quack_engine::drafts::PageSaved::Applied(applied) = applied else {
        panic!("{applied:?}");
    };
    h.settle().await?;
    let saved = items(&h, applied.case_id()).await?;
    assert_eq!(
        saved.iter().map(|e| (e.kind, e.state)).collect::<Vec<_>>(),
        vec![(EvidenceKind::Upload, EvidenceState::Saved)]
    );
    Ok(())
}
