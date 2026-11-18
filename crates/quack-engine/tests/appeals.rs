//! Appeals (§8, §20.1.1 scenario 14).

mod common;

use std::sync::Arc;

use anyhow::{Context as _, Result};
use common::{logged, void};
use quack_core::DomainError;
use quack_core::appeal::{AppealDecision, AppealState};
use quack_core::case::VoidCause;
use quack_core::ids::{AppealId, CaseId};
use quack_core::modlog::ModLogKind;
use quack_engine::Origin;
use quack_engine::appeals::{AppealError, add_information, decide, get, submit};
use quack_testkit::rules_for_tests::{levels, warning};
use quack_testkit::{GUILD, Harness, MEMBER, MEMBER_2, sf};

async fn appealed(h: &Harness) -> Result<(CaseId, AppealId)> {
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let s = submit(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "I didn't do it.",
        &h.key("apm"),
    )
    .await?;
    Ok((case, s.appeal))
}

async fn state(h: &Harness, appeal: AppealId) -> Result<(AppealState, u32)> {
    let v = get(&h.engine, &h.moderator(), appeal).await?;
    Ok((v.state, v.version))
}

async fn decision(
    h: &Harness,
    appeal: AppealId,
    d: AppealDecision,
    reason: &str,
    version: u32,
) -> Result<u32, AppealError> {
    decide(
        &h.engine,
        &h.moderator(),
        appeal,
        d,
        reason,
        version,
        &h.key("aam"),
    )
    .await
    .map(|d| d.version)
}

async fn case_voided(h: &Harness, case: CaseId) -> Result<Option<VoidCause>> {
    let mut conn = h.db.db.conn().await?;
    let c = quack_db::cases::get(&mut conn, h.guild, case)
        .await?
        .context("case")?;
    Ok(c.void.map(|v| v.cause))
}

#[tokio::test]
async fn every_legal_transition() -> Result<()> {
    let h = Harness::new().await?;
    let (case, appeal) = appealed(&h).await?;
    assert_eq!(state(&h, appeal).await?, (AppealState::Pending, 1));
    let v = decision(&h, appeal, AppealDecision::RequestInformation, "When?", 1).await?;
    assert_eq!(state(&h, appeal).await?.0, AppealState::NeedsInformation);
    let v = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "Yesterday.",
        v,
        &h.key("i"),
    )
    .await?
    .version;
    let v = decision(&h, appeal, AppealDecision::Reject, "It stands.", v).await?;
    assert_eq!(state(&h, appeal).await?.0, AppealState::Rejected);
    let v = decision(&h, appeal, AppealDecision::Reopen, "Tell us more.", v).await?;
    assert_eq!(state(&h, appeal).await?.0, AppealState::NeedsInformation);
    let v = decision(&h, appeal, AppealDecision::Close, "Closing.", v).await?;
    let v = decision(&h, appeal, AppealDecision::Reopen, "Again.", v).await?;
    let v = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "More.",
        v,
        &h.key("i"),
    )
    .await?
    .version;
    decision(&h, appeal, AppealDecision::Accept, "Fair enough.", v).await?;
    assert_eq!(state(&h, appeal).await?.0, AppealState::Accepted);
    assert_eq!(
        case_voided(&h, case).await?,
        Some(VoidCause::AppealAccepted)
    );
    let mut conn = h.db.db.conn().await?;
    let c = quack_db::cases::get(&mut conn, h.guild, case)
        .await?
        .context("case")?;
    assert_eq!(
        c.void.map(|v| v.reason).as_deref(),
        Some("Appeal accepted.")
    );
    let events = get(&h.engine, &h.moderator(), appeal).await?.events;
    assert_eq!(events.last().map(|e| e.text.as_str()), Some("Fair enough."));
    assert_eq!(logged(&h, ModLogKind::AppealAccepted).await?, 1);
    assert_eq!(logged(&h, ModLogKind::CaseVoided).await?, 1);
    h.settle().await?;
    // One DM per staff decision: request, reject, reopen, close, reopen, accept.
    assert_eq!(h.fake.world(|w| w.dms_to(sf(MEMBER)).len()), 6);
    Ok(())
}

#[tokio::test]
async fn illegal_transitions_are_refused() -> Result<()> {
    let h = Harness::new().await?;
    let (case, appeal) = appealed(&h).await?;
    let stale = decision(&h, appeal, AppealDecision::Reject, "x", 7).await;
    assert!(matches!(
        stale,
        Err(AppealError::Refused(DomainError::AppealChanged))
    ));
    let reopen = decision(&h, appeal, AppealDecision::Reopen, "x", 1).await;
    assert!(matches!(
        reopen,
        Err(AppealError::Refused(DomainError::AppealChanged))
    ));
    let reply = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "?",
        1,
        &h.key("i"),
    )
    .await;
    assert!(matches!(
        reply,
        Err(AppealError::Refused(DomainError::AppealChanged))
    ));
    let v = decision(&h, appeal, AppealDecision::Accept, "ok", 1).await?;
    for d in [
        AppealDecision::Accept,
        AppealDecision::Reject,
        AppealDecision::Close,
        AppealDecision::RequestInformation,
        AppealDecision::Reopen,
    ] {
        let again = decision(&h, appeal, d, "again", v).await;
        assert!(
            matches!(again, Err(AppealError::Refused(DomainError::CaseVoided))),
            "{d:?}"
        );
    }
    let empty = decision(&h, appeal, AppealDecision::Reject, "  ", v).await;
    assert!(matches!(empty, Err(AppealError::ReasonLength)));
    Ok(())
}

#[tokio::test]
async fn an_old_reply_is_refused() -> Result<()> {
    let h = Harness::new().await?;
    let (case, appeal) = appealed(&h).await?;
    let v1 = decision(&h, appeal, AppealDecision::RequestInformation, "First?", 1).await?;
    let v2 = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "One.",
        v1,
        &h.key("i"),
    )
    .await?
    .version;
    decision(
        &h,
        appeal,
        AppealDecision::RequestInformation,
        "Second?",
        v2,
    )
    .await?;
    // A reply written against the first request.
    let old = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "Late.",
        v1,
        &h.key("i"),
    )
    .await;
    assert!(matches!(
        old,
        Err(AppealError::Refused(DomainError::AppealChanged))
    ));
    Ok(())
}

#[tokio::test]
async fn eligibility() -> Result<()> {
    let h = Harness::new().await?;
    let (case, _) = appealed(&h).await?;
    let again = submit(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "x",
        &h.key("apm"),
    )
    .await;
    assert!(matches!(
        again,
        Err(AppealError::Refused(DomainError::AppealExists))
    ));
    let other = submit(
        &h.engine,
        sf(MEMBER_2),
        Origin::Dashboard,
        case,
        "x",
        &h.key("apm"),
    )
    .await;
    assert!(matches!(
        other,
        Err(AppealError::Refused(DomainError::CaseNotFound))
    ));
    let mut def = warning("Final");
    def.appealable = false;
    let rule = h.rule(def).await?;
    let closed = h.apply(MEMBER, rule).await?.case.id;
    let no = submit(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        closed,
        "x",
        &h.key("apm"),
    )
    .await;
    assert!(matches!(
        no,
        Err(AppealError::Refused(DomainError::AppealNotAllowed))
    ));
    let rule2 = h.rule(warning("Other")).await?;
    let voided = h.apply(MEMBER, rule2).await?.case.id;
    void(&h, voided).await?;
    let late = submit(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        voided,
        "x",
        &h.key("apm"),
    )
    .await;
    assert!(matches!(
        late,
        Err(AppealError::Refused(DomainError::CaseVoided))
    ));
    Ok(())
}

#[tokio::test]
async fn a_void_closes_the_open_appeal_and_reopen_is_refused() -> Result<()> {
    let h = Harness::new().await?;
    let (case, appeal) = appealed(&h).await?;
    void(&h, case).await?;
    let (s, v) = state(&h, appeal).await?;
    assert_eq!(s, AppealState::Closed);
    let events = get(&h.engine, &h.moderator(), appeal).await?.events;
    assert!(
        events
            .last()
            .is_some_and(|e| e.closed_by_void && e.actor_id.is_none())
    );
    let reopen = decision(&h, appeal, AppealDecision::Reopen, "x", v).await;
    assert!(matches!(
        reopen,
        Err(AppealError::Refused(DomainError::CaseVoided))
    ));
    h.settle().await?;
    assert_eq!(h.fake.world(|w| w.dms_to(sf(MEMBER)).len()), 1);
    Ok(())
}

#[tokio::test]
async fn a_submit_racing_a_void_never_leaves_an_open_appeal_on_a_voided_case() -> Result<()> {
    let h = Arc::new(Harness::with_connections(20).await?);
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    for _ in 0..10 {
        let case = h.apply(MEMBER, rule).await?.case.id;
        let (a, b) = (Arc::clone(&h), Arc::clone(&h));
        let (submitted, voided) = tokio::join!(
            tokio::spawn(async move {
                submit(
                    &a.engine,
                    sf(MEMBER),
                    Origin::Dashboard,
                    case,
                    "x",
                    &a.key("apm"),
                )
                .await
            }),
            tokio::spawn(async move { void(&b, case).await }),
        );
        voided??;
        if let Ok(s) = submitted? {
            let (state, _) = state(&h, s.appeal).await?;
            assert_eq!(state, AppealState::Closed, "the void closed it");
        }
    }
    let open = h
        .db
        .count(&format!(
            "SELECT count(*) FROM appeals a JOIN cases c ON c.id = a.case_id
             WHERE c.guild_id = {GUILD} AND c.voided_at IS NOT NULL AND a.state IN ('pending', 'needs_information')"
        ))
        .await?;
    assert_eq!(open, 0);
    Ok(())
}

#[tokio::test]
async fn accept_racing_a_direct_void_voids_once() -> Result<()> {
    let h = Arc::new(Harness::with_connections(20).await?);
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    for _ in 0..8 {
        let case = h.apply(MEMBER, rule).await?.case.id;
        let appeal = submit(
            &h.engine,
            sf(MEMBER),
            Origin::Dashboard,
            case,
            "x",
            &h.key("apm"),
        )
        .await?
        .appeal;
        let (a, b) = (Arc::clone(&h), Arc::clone(&h));
        let (accepted, voided) = tokio::join!(
            tokio::spawn(
                async move { decision(&a, appeal, AppealDecision::Accept, "ok", 1).await }
            ),
            tokio::spawn(async move { void(&b, case).await }),
        );
        let (accepted, voided) = (accepted?.is_ok(), voided?.is_ok());
        assert!(accepted != voided, "exactly one of them voids the case");
        let (s, _) = state(&h, appeal).await?;
        assert_eq!(
            s,
            if accepted {
                AppealState::Accepted
            } else {
                AppealState::Closed
            }
        );
    }
    let voids =
        h.db.count("SELECT count(*) FROM mod_log WHERE kind = 'case_voided'")
            .await?;
    assert_eq!(voids, 8);
    Ok(())
}

#[tokio::test]
async fn two_quick_decisions_send_two_dms_with_their_own_text() -> Result<()> {
    let h = Harness::new().await?;
    let (case, appeal) = appealed(&h).await?;
    let v = decision(
        &h,
        appeal,
        AppealDecision::RequestInformation,
        "First reason.",
        1,
    )
    .await?;
    let v = add_information(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "Reply.",
        v,
        &h.key("i"),
    )
    .await?
    .version;
    decision(&h, appeal, AppealDecision::Reject, "Second reason.", v).await?;
    h.settle().await?;
    let texts: Vec<String> = h.fake.world(|w| {
        w.dms_to(sf(MEMBER))
            .iter()
            .filter_map(|m| m.msg.as_ref().map(quack_text::Msg::plain))
            .collect()
    });
    assert_eq!(texts.len(), 2);
    assert!(texts[0].contains("First reason.") && !texts[0].contains("Second reason."));
    assert!(texts[1].contains("Second reason.") && !texts[1].contains("First reason."));
    Ok(())
}

/// Engine review MAJOR 6: an appeal submitted while a change of the appeal
/// channel is in flight gets its post in the new channel. The test holds
/// the settings row as a settings change does, sets the channel, and
/// commits only after the submission has started.
#[tokio::test]
async fn an_appeal_during_a_channel_change_posts_in_the_new_channel() -> Result<()> {
    let h = Arc::new(Harness::new().await?);
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, true));
    let mut change = h.db.db.begin().await?;
    quack_db::settings::lock(&mut change, h.guild).await?;
    sqlx::query("UPDATE guild_settings SET appeal_channel_id = $1 WHERE guild_id = $2")
        .bind(i64::from(channel))
        .bind(i64::try_from(GUILD)?)
        .execute(&mut *change)
        .await?;
    let key = h.key("apm");
    let submitting = {
        let h = Arc::clone(&h);
        tokio::spawn(async move {
            submit(
                &h.engine,
                sf(MEMBER),
                Origin::Dashboard,
                case,
                "Not me.",
                &key,
            )
            .await
        })
    };
    let waiting = "SELECT count(*) FROM pg_stat_activity
                   WHERE datname = current_database() AND wait_event_type = 'Lock'";
    let blocked = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while h.db.count(waiting).await? < 1 && !submitting.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        anyhow::Ok(())
    })
    .await;
    assert!(blocked.is_ok());
    change.commit().await?;
    let appeal = submitting.await??.appeal;
    let mut conn = h.db.db.conn().await?;
    let post = quack_db::messages::by_owner(
        &mut conn,
        h.guild,
        quack_db::messages::Owner::Appeal(appeal),
    )
    .await?
    .context("the appeal's post")?;
    assert_eq!(post.channel, Some(channel));
    Ok(())
}

/// Engine review MAJOR 7: the appeal channel still configured but gone
/// (its delete event was missed) ends the sync with an error instead of
/// a new generation every run.
#[tokio::test]
async fn a_post_whose_configured_channel_is_gone_stops_trying() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(levels("Spam", &[None], false)).await?;
    let case = h.apply(MEMBER, rule).await?.case.id;
    let channel = h.fake.world(|w| w.add_text_channel(h.guild, true));
    sqlx::query("UPDATE guild_settings SET appeal_channel_id = $1 WHERE guild_id = $2")
        .bind(i64::from(channel))
        .bind(i64::try_from(GUILD)?)
        .execute(h.db.db.pool())
        .await?;
    let appeal = submit(
        &h.engine,
        sf(MEMBER),
        Origin::Dashboard,
        case,
        "Not me.",
        &h.key("apm"),
    )
    .await?
    .appeal;
    h.settle().await?;
    h.fake.world(|w| {
        if let Some(g) = w.guilds.get_mut(&h.guild) {
            g.channels.remove(&channel);
        }
    });
    let mut tx = h.db.db.begin().await?;
    quack_db::messages::bump(&mut tx, h.guild, quack_db::messages::Owner::Appeal(appeal)).await?;
    tx.commit().await?;
    h.settle().await?;
    let mut conn = h.db.db.conn().await?;
    let post = quack_db::messages::by_owner(
        &mut conn,
        h.guild,
        quack_db::messages::Owner::Appeal(appeal),
    )
    .await?
    .context("the appeal's post")?;
    assert_eq!(post.last_error.as_deref(), Some("destination_unavailable"));
    assert_eq!(post.generation, 1, "no new generation per run");
    Ok(())
}
