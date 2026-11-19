//! Evidence (§7, §20.1.1 scenario 16) and context edits (§4.9, scenario 19).

mod common;

use anyhow::{Context as _, Result};
use quack_core::DomainError;
use quack_core::case::ContextInput;
use quack_core::evidence::{EvidenceFailure, EvidenceState, FileCopyState};
use quack_core::ids::{ChannelId, MessageId};
use quack_core::permissions::Permission;
use quack_core::rule::{Field, FieldKey, FieldKind};
use quack_db::evidence::Evidence;
use quack_db::jobs::JobKind;
use quack_engine::CaseRef;
use quack_engine::cases::{CaseError, add_evidence, edit_context};
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, GuildSetup, Script};
use quack_testkit::rules_for_tests::{levels, warning};
use quack_testkit::{GUILD, Harness, MEMBER, sf};

fn link(channel: ChannelId, message: MessageId) -> String {
    format!("https://discord.com/channels/{GUILD}/{channel}/{message}")
}

async fn items(h: &Harness, case: quack_core::ids::CaseId) -> Result<Vec<Evidence>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::evidence::for_case(&mut conn, h.guild, case).await?)
}

async fn files(
    h: &Harness,
    case: quack_core::ids::CaseId,
) -> Result<Vec<quack_db::evidence::EvidenceFile>> {
    let mut conn = h.db.db.conn().await?;
    Ok(quack_db::evidence::files_for_case(&mut conn, h.guild, case).await?)
}

/// A case, and a message in a public channel to add as evidence.
async fn setup(h: &Harness) -> Result<(quack_core::ids::CaseId, ChannelId)> {
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    Ok((case, channel))
}

async fn add(h: &Harness, case: quack_core::ids::CaseId, links: &[String]) -> Result<()> {
    add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        links,
        &h.key("eam"),
    )
    .await?;
    Ok(())
}

fn evidence_channel(h: &Harness) -> ChannelId {
    h.fake.world(|w| w.add_text_channel(h.guild, true))
}

async fn set_evidence_channel(h: &Harness, channel: ChannelId) -> Result<()> {
    h.db.execute(&format!(
        "UPDATE guild_settings SET evidence_channel_id = {channel} WHERE guild_id = {GUILD}"
    ))
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_message_is_saved_with_its_files_linked() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let message = h.fake.world(|w| {
        let a = w.attachment("a.png", Some("image/png"), vec![1, 2, 3]);
        w.post(h.guild, channel, sf(MEMBER), "buy cheap things", vec![a])
    });
    add(&h, case, &[link(channel, message)]).await?;
    h.settle().await?;
    let saved = items(&h, case).await?;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].state, EvidenceState::Saved);
    assert_eq!(saved[0].content.as_deref(), Some("buy cheap things"));
    assert_eq!(saved[0].author, Some(sf(MEMBER)));
    let f = files(&h, case).await?;
    assert_eq!(f[0].copy_state, FileCopyState::LinkOnly);
    Ok(())
}

#[tokio::test]
async fn files_are_copied_to_a_staff_only_evidence_channel() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let copies = evidence_channel(&h);
    set_evidence_channel(&h, copies).await?;
    let message = h.fake.world(|w| {
        let a = w.attachment("a.png", Some("image/png"), vec![1, 2, 3]);
        let b = w.attachment("b.exe", Some("application/x-msdownload"), vec![4]);
        let c = w.attachment("big.mp4", Some("video/mp4"), vec![0; 10]);
        w.post(h.guild, channel, sf(MEMBER), "x", vec![a, b, c])
    });
    // The third file claims 11 MiB.
    h.fake.world(|w| {
        if let Some(m) = w.messages.get_mut(&message)
            && let Some(f) = m.fetched.attachments.get_mut(2)
        {
            f.size_bytes = 11 * 1024 * 1024;
        }
    });
    add(&h, case, &[link(channel, message)]).await?;
    h.settle().await?;
    let states: Vec<FileCopyState> = files(&h, case)
        .await?
        .iter()
        .map(|f| f.copy_state)
        .collect();
    assert_eq!(
        states,
        vec![
            FileCopyState::Copied,
            FileCopyState::UnsupportedType,
            FileCopyState::TooLarge
        ]
    );
    let sent = h.fake.world(|w| {
        w.sent_to(copies)
            .iter()
            .map(|m| m.fetched.attachments[0].filename.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(sent, vec!["SPOILER_a.png".to_owned()]);
    Ok(())
}

#[tokio::test]
async fn a_crash_between_copies_resumes_at_the_next_file() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let copies = evidence_channel(&h);
    set_evidence_channel(&h, copies).await?;
    let message = h.fake.world(|w| {
        let a = w.attachment("a.png", Some("image/png"), vec![1]);
        let b = w.attachment("b.png", Some("image/png"), vec![2]);
        w.post(h.guild, channel, sf(MEMBER), "x", vec![a, b])
    });
    add(&h, case, &[link(channel, message)]).await?;
    // The first upload goes through; the second takes effect and the
    // process dies before it is recorded.
    let gate = quack_testkit::fake::Gate::new();
    h.fake.script(Call::Send, Script::Pass);
    h.fake.script(Call::Send, Script::HoldAfter(gate.clone()));
    h.crash_at(JobKind::CaptureEvidence, &gate).await?;
    h.settle().await?;
    let states: Vec<FileCopyState> = files(&h, case)
        .await?
        .iter()
        .map(|f| f.copy_state)
        .collect();
    assert_eq!(states, vec![FileCopyState::Copied, FileCopyState::Copied]);
    assert_eq!(
        h.fake.world(|w| w.sent_to(copies).len()),
        2,
        "no duplicate copy"
    );
    Ok(())
}

#[tokio::test]
async fn capture_failures_have_their_reasons() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    // Deleted.
    let gone = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
    h.fake.world(|w| {
        w.messages.remove(&gone);
    });
    // A link naming this server but a channel of another one: the
    // channel's server decides, not the link (or the fetched message,
    // which names none).
    let elsewhere: quack_core::ids::GuildId = sf(999);
    let (far, other) = h.fake.world(|w| {
        w.add_guild(&GuildSetup {
            id: elsewhere,
            owner: sf(MEMBER),
            quack_position: 50,
            quack_permissions: Permission::Administrator.into(),
            mfa_required: false,
        });
        let far = w.add_text_channel(elsewhere, false);
        (far, w.post(elsewhere, far, sf(MEMBER), "z", vec![]))
    });
    add(&h, case, &[link(channel, gone), link(far, other)]).await?;
    h.settle().await?;
    let failures: Vec<Option<EvidenceFailure>> =
        items(&h, case).await?.iter().map(|e| e.failure).collect();
    assert!(failures.contains(&Some(EvidenceFailure::MessageDeleted)));
    assert!(failures.contains(&Some(EvidenceFailure::OtherServer)));
    Ok(())
}

#[tokio::test]
async fn quack_without_access_and_a_moderator_without_access() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
    h.fake.script(
        Call::FetchMessage,
        Script::refuse(RefusalKind::MissingAccess),
    );
    add(&h, case, &[link(channel, m)]).await?;
    h.settle().await?;
    assert_eq!(
        items(&h, case).await?[0].failure,
        Some(EvidenceFailure::QuackNoAccess)
    );
    // A channel the moderator can't see.
    let secret = h.fake.world(|w| {
        let c = w.add_text_channel(h.guild, true);
        w.post(h.guild, c, sf(MEMBER), "secret", vec![]);
        c
    });
    let s = h
        .fake
        .world(|w| w.post(h.guild, secret, sf(MEMBER), "s", vec![]));
    add(&h, case, &[link(secret, s)]).await?;
    h.settle().await?;
    let found = items(&h, case).await?;
    let item = found.iter().find(|e| e.message == s).context("item")?;
    assert_eq!(item.failure, Some(EvidenceFailure::ModeratorNoAccess));
    // A private thread the moderator isn't in, under a channel they see.
    let thread = h.fake.world(|w| {
        let id = ChannelId::new(w.next_id()).unwrap_or_else(|| unreachable!());
        if let Some(g) = w.guilds.get_mut(&h.guild)
            && let Some(parent) = g.channels.get(&channel).cloned()
        {
            let mut t = parent;
            t.id = id;
            t.kind = 12;
            t.parent_id = Some(channel);
            g.channels.insert(id, t);
        }
        id
    });
    let t = h
        .fake
        .world(|w| w.post(h.guild, thread, sf(MEMBER), "t", vec![]));
    add(&h, case, &[link(thread, t)]).await?;
    h.settle().await?;
    let found = items(&h, case).await?;
    let item = found.iter().find(|e| e.message == t).context("item")?;
    assert_eq!(item.failure, Some(EvidenceFailure::ModeratorNoAccess));
    Ok(())
}

#[tokio::test]
async fn a_failed_item_is_captured_again_when_added_again() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
    h.fake.script(
        Call::FetchMessage,
        Script::refuse(RefusalKind::MissingAccess),
    );
    add(&h, case, &[link(channel, m)]).await?;
    h.settle().await?;
    assert_eq!(items(&h, case).await?[0].state, EvidenceState::Failed);
    let again = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &[link(channel, m)],
        &h.key("eam"),
    )
    .await?;
    assert_eq!((again.new, again.retrying, again.already_saved), (0, 1, 0));
    h.settle().await?;
    assert_eq!(items(&h, case).await?[0].state, EvidenceState::Saved);
    let once_more = add_evidence(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case),
        &[link(channel, m)],
        &h.key("eam"),
    )
    .await?;
    assert_eq!(once_more.already_saved, 1);
    Ok(())
}

#[tokio::test]
async fn the_cap_holds_under_concurrent_adds() -> Result<()> {
    let h = std::sync::Arc::new(Harness::with_connections(30).await?);
    let (case, channel) = setup(&h).await?;
    let mut batches = Vec::new();
    for _ in 0..6 {
        let links: Vec<String> = (0..10)
            .map(|_| {
                let m = h
                    .fake
                    .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
                link(channel, m)
            })
            .collect();
        batches.push(links);
    }
    let mut tasks = Vec::new();
    for links in batches {
        let h = std::sync::Arc::clone(&h);
        tasks.push(tokio::spawn(async move {
            add_evidence(
                &h.engine,
                &h.moderator(),
                CaseRef::Id(case),
                &links,
                &h.key("eam"),
            )
            .await
        }));
    }
    let mut refused = 0;
    for task in tasks {
        match task.await? {
            Ok(_) => {}
            Err(CaseError::Refused(DomainError::EvidenceLimitReached { .. })) => refused += 1,
            Err(e) => return Err(e.into()),
        }
    }
    assert_eq!(refused, 1);
    assert_eq!(items(&h, case).await?.len(), 50);
    Ok(())
}

#[tokio::test]
async fn two_editors_at_one_revision_and_links_become_evidence() -> Result<()> {
    let h = Harness::new().await?;
    let mut def = warning("Spam");
    def.fields = vec![Field {
        key: FieldKey("what".to_owned()),
        label: "What happened?".to_owned(),
        kind: FieldKind::LongText,
        required: false,
    }];
    let rule = h.rule(def).await?;
    let case = h.apply(MEMBER, rule).await?.case;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, false));
    let m = h
        .fake
        .world(|w| w.post(h.guild, channel, sf(MEMBER), "x", vec![]));
    let text = format!("They posted {} and left.", link(channel, m));
    let first = edit_context(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case.id),
        &[("what".to_owned(), ContextInput::Text(text))],
        1,
        &h.key("cem"),
    )
    .await?;
    assert_eq!((first.revision, first.new_evidence), (2, 1));
    let second = edit_context(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case.id),
        &[(
            "what".to_owned(),
            ContextInput::Text("Something else.".to_owned()),
        )],
        1,
        &h.key("cem"),
    )
    .await;
    assert!(matches!(second, Err(CaseError::ContextChanged)));
    assert_eq!(items(&h, case.id).await?.len(), 1);
    Ok(())
}

/// A message with `n` image attachments in a public channel.
fn post_images(h: &Harness, channel: ChannelId, n: usize) -> MessageId {
    h.fake.world(|w| {
        let files = (0..n)
            .map(|i| w.attachment(&format!("{i}.png"), Some("image/png"), vec![1, 2, 3]))
            .collect();
        w.post(h.guild, channel, sf(MEMBER), "x", files)
    })
}

/// Engine review MAJOR 5: the destination is checked again right before
/// every upload. A channel that stops being staff-only after the first
/// upload receives nothing more.
#[tokio::test]
async fn uploads_stop_when_the_evidence_channel_opens_mid_copy() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let copies = evidence_channel(&h);
    set_evidence_channel(&h, copies).await?;
    let message = post_images(&h, channel, 3);
    add(&h, case, &[link(channel, message)]).await?;
    let gate = h.fake.hold_after(Call::Send);
    let task = h.spawn_one(JobKind::CaptureEvidence).await?;
    gate.reached().await;
    // The first copy is posted; now everyone can see the channel.
    h.fake.world(|w| {
        if let Some(c) = w
            .guilds
            .get_mut(&h.guild)
            .and_then(|g| g.channels.get_mut(&copies))
        {
            c.overwrites.clear();
        }
    });
    gate.release();
    task.await?;
    h.settle().await?;
    assert_eq!(h.fake.count(Call::Send), 1, "no upload after the change");
    let states: Vec<FileCopyState> = files(&h, case)
        .await?
        .iter()
        .map(|f| f.copy_state)
        .collect();
    assert_eq!(
        states,
        vec![
            FileCopyState::Copied,
            FileCopyState::CopyFailed,
            FileCopyState::CopyFailed
        ]
    );
    assert!(items(&h, case).await?[0].copy_skipped_not_staff_only);
    Ok(())
}

/// Engine review MAJOR 17: a file's three upload tries hold across runs.
/// A run that stops after an unknown upload doesn't give the next run
/// three more.
#[tokio::test]
async fn a_files_upload_budget_holds_across_runs() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let copies = evidence_channel(&h);
    set_evidence_channel(&h, copies).await?;
    let message = post_images(&h, channel, 1);
    add(&h, case, &[link(channel, message)]).await?;
    // Two unknown uploads, then the run dies at the third.
    h.fake.script_n(Call::Send, 2, &Script::Unknown);
    let gate = h.fake.hold(Call::Send);
    h.crash_at(JobKind::CaptureEvidence, &gate).await?;
    h.fake.clear_scripts();
    h.fake.script_n(Call::Send, 5, &Script::Unknown);
    h.settle().await?;
    assert_eq!(h.fake.count(Call::Send), 3, "three tries in all");
    assert_eq!(
        files(&h, case).await?[0].copy_state,
        FileCopyState::CopyFailed
    );
    Ok(())
}

/// Engine review MAJOR 17: a rate-limited upload ends the run with
/// Discord's delay instead of trying again at once, and doesn't use a try.
#[tokio::test]
async fn a_rate_limited_upload_waits_and_keeps_its_tries() -> Result<()> {
    let h = Harness::new().await?;
    let (case, channel) = setup(&h).await?;
    let copies = evidence_channel(&h);
    set_evidence_channel(&h, copies).await?;
    let message = post_images(&h, channel, 1);
    add(&h, case, &[link(channel, message)]).await?;
    h.fake.script_n(
        Call::Send,
        2,
        &Script::RetryAfter(std::time::Duration::from_secs(600)),
    );
    let ran = h.run_one(JobKind::CaptureEvidence).await?.context("ran")?;
    assert_eq!(h.fake.count(Call::Send), 1, "one call, then the run ends");
    assert!(
        matches!(
            ran.end,
            quack_testkit::RunEnd::Settled(quack_db::jobs::JobResult::Retry { .. })
        ),
        "{:?}",
        ran.end
    );
    h.settle().await?;
    // Two rate limits used no tries: the third call copies it.
    assert_eq!(h.fake.count(Call::Send), 3);
    assert_eq!(files(&h, case).await?[0].copy_state, FileCopyState::Copied);
    Ok(())
}

/// Engine review MAJOR 20: the same request with its context fields in
/// another order is the same request.
#[tokio::test]
async fn a_replay_with_reordered_fields_is_the_same_request() -> Result<()> {
    let h = Harness::new().await?;
    let mut def = warning("Spam");
    let field = |key: &str, kind: FieldKind| Field {
        key: FieldKey(key.to_owned()),
        label: key.to_owned(),
        kind,
        required: false,
    };
    def.fields = vec![
        field("what", FieldKind::LongText),
        field("where", FieldKind::ShortText),
    ];
    let rule = h.rule(def).await?;
    let case = h.apply(MEMBER, rule).await?.case;
    let what = ("what".to_owned(), ContextInput::Text("Spam.".to_owned()));
    let place = (
        "where".to_owned(),
        ContextInput::Text("#general".to_owned()),
    );
    let key = h.key("cem");
    let first = edit_context(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case.id),
        &[what.clone(), place.clone()],
        1,
        &key,
    )
    .await?;
    let again = edit_context(
        &h.engine,
        &h.moderator(),
        CaseRef::Id(case.id),
        &[place, what],
        1,
        &key,
    )
    .await?;
    assert!(again.replayed);
    assert_eq!(again.case, first.case);
    Ok(())
}
