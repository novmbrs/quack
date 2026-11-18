//! Servers, rules, settings, sessions, drafts, modules, statistics, and
//! housekeeping.

mod common;

use std::time::Duration;

use anyhow::{Context as _, Result};
use common::{account_id, case, db, definition, guild, guild_id, member_id, rule};
use quack_core::ids::{ChannelId, GuildId, MessageId, RoleId, RuleId, TicketId};
use quack_core::modules::{ChannelSetting, ModuleName, TicketState, TranscriptPartial};
use quack_db::guilds::{self, GuildFacts};
use quack_db::sessions::{self, NewSession, OauthState};
use quack_db::settings::{self, SettingsPatch};
use quack_db::{Conflict, drafts, honeypot, housekeeping, rules, stats, tickets};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::OffsetDateTime;
use uuid::Uuid;

fn channel(n: u64) -> ChannelId {
    ChannelId::new(n).unwrap_or_else(|| unreachable!())
}

fn facts(id: GuildId) -> GuildFacts {
    GuildFacts {
        id,
        name: "Ducks".to_owned(),
        icon_hash: None,
        owner_id: account_id(1),
        mfa_required: true,
    }
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn joining_leaving_and_returning(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild_id(42);
    let mut conn = db.conn().await?;
    let first = guilds::upsert(&mut conn, &facts(g)).await?;
    assert!(first.new && !first.returned);
    assert!(guilds::ensure_children(&mut conn, g).await?);
    assert!(
        !guilds::ensure_children(&mut conn, g).await?,
        "only the first join creates settings"
    );
    assert!(guilds::mark_left(&mut conn, g).await?);
    assert!(!guilds::mark_left(&mut conn, g).await?);
    let back = guilds::upsert(&mut conn, &facts(g)).await?;
    assert!(!back.new && back.returned);
    assert_eq!(guilds::installed(&mut conn).await?, vec![g]);
    assert_eq!(guilds::next_case_number(&mut conn, g).await?, 1);
    let stored = guilds::get(&mut conn, g).await?.context("guild")?;
    assert!(stored.installed && stored.mfa_required && stored.left_at.is_none());
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn rule_versions_are_added_by_compare_and_set(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, first) = rule(&db, g, "Spam").await?;
    let mut tx = db.begin().await?;
    let lock = rules::lock_for_update(&mut tx, g, spam)
        .await?
        .context("rule")?;
    assert_eq!((lock.current_version, lock.archived), (1, false));
    let edited = definition("Spam v2", &[None, None])?;
    assert_eq!(
        rules::add_version(&mut tx, g, spam, 1, &edited, Some(account_id(3))).await?,
        Some(2)
    );
    assert_eq!(
        rules::add_version(&mut tx, g, spam, 1, &edited, None).await?,
        None,
        "stale version"
    );
    tx.commit().await?;

    let mut conn = db.conn().await?;
    let current = rules::get(&mut conn, g, spam).await?.context("rule")?;
    assert_eq!(
        (current.current_version, current.definition.clone()),
        (2, edited)
    );
    assert_eq!(
        rules::version(&mut conn, g, spam, 1)
            .await?
            .map(|v| v.definition),
        Some(first)
    );
    assert_eq!(rules::versions(&mut conn, g, spam).await?.len(), 2);
    assert!(
        rules::get(&mut conn, guild_id(9), spam).await?.is_none(),
        "rules never cross servers"
    );

    // Names are unique among active rules only.
    let mut tx = db.begin().await?;
    let clash = rules::create(
        &mut tx,
        &rules::NewRule {
            id: RuleId::from_uuid(Uuid::now_v7()),
            guild: g,
            definition: definition("SPAM V2", &[None])?,
            created_by: None,
            reconstructed: false,
        },
    )
    .await;
    assert_eq!(
        clash.err().and_then(|e| e.conflict()),
        Some(Conflict::RuleNameTaken)
    );
    drop(tx);
    assert!(rules::archive(&mut conn, g, spam, None).await?);
    assert!(!rules::archive(&mut conn, g, spam, None).await?);
    assert_eq!(rules::count_active(&mut conn, g).await?, 0);
    assert_eq!(rules::load_cache(&mut conn, &[g]).await?, Vec::new());
    assert_eq!(rules::list(&mut conn, g, true).await?.len(), 1);
    assert!(rules::restore(&mut conn, g, spam, None).await?);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn settings_write_only_sent_fields_and_compare_versions(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let mut conn = db.conn().await?;
    let before = settings::get(&mut conn, g).await?.context("settings")?;
    let role = RoleId::new(77).unwrap_or_else(|| unreachable!());
    let patch = SettingsPatch {
        moderator_role_ids: Some(vec![role]),
        log_channel: Some(Some(channel(5))),
        dm_footer: Some(Some("Be kind".to_owned())),
        ..SettingsPatch::default()
    };
    let version = settings::update(&mut conn, g, &patch, before.version, account_id(3)).await?;
    assert_eq!(version, Some(before.version + 1));
    assert_eq!(
        settings::update(&mut conn, g, &patch, before.version, account_id(3)).await?,
        None
    );
    let clear_footer = SettingsPatch {
        dm_footer: Some(None),
        ..SettingsPatch::default()
    };
    settings::update(
        &mut conn,
        g,
        &clear_footer,
        before.version + 1,
        account_id(3),
    )
    .await?;
    let after = settings::get(&mut conn, g).await?.context("settings")?;
    assert_eq!(after.moderator_role_ids, vec![role]);
    assert_eq!(
        (after.log_channel, after.dm_footer),
        (Some(channel(5)), None)
    );

    assert!(!settings::clear_channel(&mut conn, g, ChannelSetting::LogChannel, channel(6)).await?);
    assert!(settings::clear_channel(&mut conn, g, ChannelSetting::LogChannel, channel(5)).await?);
    assert!(settings::acknowledge_starter_notice(&mut conn, g, account_id(3)).await?);
    assert!(!settings::acknowledge_starter_notice(&mut conn, g, account_id(3)).await?);

    sqlx::query("UPDATE honeypot_settings SET channel_id = 9, rule_id = NULL WHERE guild_id = $1")
        .bind(i64::from(g))
        .execute(&mut *conn)
        .await?;
    let on = settings::set_module_enabled(
        &mut conn,
        g,
        ModuleName::ActivityLog,
        true,
        None,
        Some(account_id(3)),
    )
    .await?;
    assert!(on.is_some());
    assert_eq!(
        settings::set_module_enabled(&mut conn, g, ModuleName::ActivityLog, true, None, None)
            .await?,
        None,
        "already on"
    );
    assert_eq!(
        settings::disable_modules_on_leave(&mut conn, g).await?,
        vec![ModuleName::ActivityLog]
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn sessions_and_login_states(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let mut conn = db.conn().await?;
    quack_db::accounts::upsert(&mut conn, account_id(5), true).await?;
    let session = NewSession {
        id_hash: [1; 32],
        account: account_id(5),
        csrf_token: "a".repeat(43),
        discord_token_ciphertext: vec![9; 40],
        key_fingerprint: [2; 8],
        expires_at: OffsetDateTime::now_utc() + time::Duration::days(7),
    };
    sessions::create(&mut conn, &session).await?;
    assert!(
        sessions::get(&mut conn, &[1; 32], &[3; 8]).await?.is_none(),
        "another secret"
    );
    let found = sessions::get(&mut conn, &[1; 32], &[2; 8])
        .await?
        .context("session")?;
    assert_eq!(found.account, account_id(5));
    assert!(
        !sessions::touch(&mut conn, &[1; 32]).await?,
        "seen less than 5 minutes ago"
    );
    assert_eq!(sessions::count_active(&mut conn).await?, 1);
    assert!(sessions::delete(&mut conn, &[1; 32]).await?);

    let state = OauthState {
        code_verifier: "v".repeat(64),
        redirect_to: "/guilds".to_owned(),
    };
    sessions::create_oauth_state(&mut conn, &[4; 32], &[5; 32], &state).await?;
    assert!(
        sessions::consume_oauth_state(&mut conn, &[4; 32], &[6; 32])
            .await?
            .is_none(),
        "another browser"
    );
    assert_eq!(
        sessions::consume_oauth_state(&mut conn, &[4; 32], &[5; 32]).await?,
        Some(state)
    );
    assert!(
        sessions::consume_oauth_state(&mut conn, &[4; 32], &[5; 32])
            .await?
            .is_none(),
        "consumed once"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn draft_pages_are_saved_once(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    let id = Uuid::now_v7();
    let draft = drafts::NewDraft {
        id,
        guild: g,
        actor: account_id(3),
        member: member_id(7),
        rule: None,
        private: true,
        receipt_channel: channel(5),
        source: Some((
            channel(5),
            MessageId::new(6).unwrap_or_else(|| unreachable!()),
        )),
    };
    let mut conn = db.conn().await?;
    assert!(drafts::create(&mut conn, &draft).await?);
    assert!(!drafts::create(&mut conn, &draft).await?);
    assert!(
        !drafts::set_rule(&mut conn, guild_id(99), id, account_id(3), spam, 1).await?,
        "another server's draft"
    );
    assert!(drafts::set_rule(&mut conn, g, id, account_id(3), spam, 1).await?);
    assert!(
        !drafts::set_rule(&mut conn, g, id, account_id(3), spam, 1).await?,
        "rule already chosen"
    );
    assert!(
        drafts::lock(&mut conn, g, account_id(4), id)
            .await?
            .is_none(),
        "another moderator's draft"
    );
    let values = serde_json::json!({ "where": "general" });
    assert!(drafts::save_page(&mut conn, g, account_id(3), id, 1, &values, true).await?);
    assert!(
        !drafts::save_page(&mut conn, g, account_id(3), id, 1, &values, true).await?,
        "double click"
    );
    let locked = drafts::lock(&mut conn, g, account_id(3), id)
        .await?
        .context("draft")?;
    assert_eq!(
        (locked.page, locked.rule, locked.values),
        (2, Some((spam, 1)), values)
    );
    assert!(
        !drafts::delete(&mut conn, g, account_id(4), id).await?,
        "another moderator's draft is not deleted"
    );
    assert!(drafts::delete(&mut conn, g, account_id(3), id).await?);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn tickets_freeze_their_journal_and_close(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let ticket = TicketId::from_uuid(Uuid::now_v7());
    let mut conn = db.conn().await?;
    tickets::insert(&mut conn, ticket, g, member_id(7), channel(5)).await?;
    assert!(
        !tickets::mark_open(&mut conn, g, ticket).await?,
        "no thread yet"
    );
    assert!(tickets::set_thread(&mut conn, g, ticket, channel(70)).await?);
    assert!(
        tickets::begin_welcome(&mut conn, g, ticket)
            .await?
            .is_some()
    );
    assert!(tickets::mark_welcome_posted(&mut conn, g, ticket).await?);
    assert!(tickets::mark_open(&mut conn, g, ticket).await?);
    let message = |n: u64| tickets::JournalMessage {
        message: MessageId::new(n).unwrap_or_else(|| unreachable!()),
        author: account_id(7),
        author_name: "duck".to_owned(),
        text: "help".to_owned(),
        files: Vec::new(),
        sent_at: OffsetDateTime::now_utc(),
    };
    assert!(tickets::add_journal_message(&mut conn, g, ticket, &message(1)).await?);
    assert!(!tickets::add_journal_message(&mut conn, g, ticket, &message(1)).await?);
    assert_eq!(tickets::mark_live_journal_gaps(&mut conn).await?, 1);
    assert_eq!(tickets::mark_live_journal_gaps(&mut conn).await?, 0);
    assert!(tickets::request_close(&mut conn, g, ticket, Some(account_id(7))).await?);
    assert!(tickets::mark_frozen(&mut conn, g, ticket).await?);
    assert!(
        !tickets::add_journal_message(&mut conn, g, ticket, &message(2)).await?,
        "frozen"
    );
    assert_eq!(
        tickets::journal(&mut conn, g, ticket, None, 10)
            .await?
            .len(),
        1
    );
    assert!(
        tickets::save_transcript(
            &mut conn,
            g,
            ticket,
            "duck: help",
            Some(TranscriptPartial::History),
            30
        )
        .await?
    );
    assert!(!tickets::save_transcript(&mut conn, g, ticket, "again", None, 30).await?);
    assert!(tickets::mark_closed(&mut conn, g, ticket, 30).await?);
    assert!(tickets::mark_thread_deleted(&mut conn, g, ticket, false).await?);
    let closed = tickets::get(&mut conn, g, ticket)
        .await?
        .context("ticket")?;
    assert_eq!(closed.state, TicketState::Closed);
    let transcript = tickets::transcript(&mut conn, g, ticket)
        .await?
        .context("transcript")?;
    assert_eq!(transcript.partial, Some(TranscriptPartial::History));
    assert!(
        tickets::live_for_member(&mut conn, g, member_id(7))
            .await?
            .is_none()
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn honeypot_hits_batch_and_burst(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (trap, _) = rule(&db, g, "Honeypot").await?;
    let m = member_id(7);
    let msg = |n: u64| MessageId::new(n).unwrap_or_else(|| unreachable!());
    let mut conn = db.conn().await?;
    assert!(honeypot::insert_hit(&mut conn, g, channel(5), msg(10), m, 1, Some(trap)).await?);
    assert!(!honeypot::insert_hit(&mut conn, g, channel(5), msg(10), m, 1, Some(trap)).await?);
    honeypot::insert_hit(&mut conn, g, channel(5), msg(11), m, 1, Some(trap)).await?;
    assert_eq!(
        honeypot::members_with_pending_hits(&mut conn, 10).await?,
        vec![(g, m)]
    );
    let one = honeypot::hit(&mut conn, g, msg(11)).await?.context("hit")?;
    assert_eq!((one.member, one.settings_version), (m, 1));
    assert!(
        honeypot::hit(&mut conn, guild_id(2), msg(11))
            .await?
            .is_none()
    );
    let batch = honeypot::pending_batch(&mut conn, g, m).await?;
    assert_eq!(
        batch.iter().map(|h| h.message).collect::<Vec<_>>(),
        vec![msg(10), msg(11)]
    );
    let c = case(&db, g, m, trap, None).await?;
    assert_eq!(
        honeypot::mark_case_created(&mut conn, g, &[msg(10), msg(11)], c.id).await?,
        2
    );
    assert_eq!(
        honeypot::mark_failed(&mut conn, g, &[msg(10)], "exempt").await?,
        0,
        "only pending hits"
    );
    let first = batch.first().map(|h| h.created_at).context("hit")?;
    let later = first + time::Duration::seconds(10);
    assert_eq!(
        honeypot::burst_case(&mut conn, g, m, 1, later).await?,
        Some(c.id)
    );
    assert_eq!(
        honeypot::burst_case(&mut conn, g, m, 2, later).await?,
        None,
        "another version"
    );
    let much_later = first + time::Duration::seconds(40);
    assert_eq!(
        honeypot::burst_case(&mut conn, g, m, 1, much_later).await?,
        None
    );
    assert!(honeypot::mark_cleanup(&mut conn, g, msg(10), false).await?);
    assert!(!honeypot::mark_cleanup(&mut conn, g, msg(10), true).await?);
    let (cleared, off) = honeypot::clear_channel(&mut conn, g, channel(5)).await?;
    assert_eq!(
        (cleared, off),
        (false, false),
        "the channel isn't the honeypot's"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn statistics_and_housekeeping(_: PgPoolOptions, options: PgConnectOptions) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (spam, _) = rule(&db, g, "Spam").await?;
    case(&db, g, member_id(7), spam, None).await?;
    case(&db, g, member_id(8), spam, None).await?;
    let now = OffsetDateTime::now_utc();
    let s = stats::compute(
        &db,
        g,
        now - time::Duration::days(1),
        now + time::Duration::minutes(1),
    )
    .await?;
    assert_eq!(s.cases_total, 2);
    assert_eq!(
        s.cases_by_rule,
        vec![(Some(spam), Some("Spam".to_owned()), 2)]
    );
    assert_eq!(s.cases_by_voided, vec![(false, 2)]);

    let mut conn = db.conn().await?;
    quack_db::accounts::upsert(&mut conn, account_id(5), false).await?;
    for (hash, fingerprint, expires) in [
        ([1_u8; 32], [2_u8; 8], "1 day"),
        ([2; 32], [2; 8], "-1 day"),
        ([3; 32], [9; 8], "1 day"),
    ] {
        sqlx::query(
            "INSERT INTO sessions (id_hash, account_id, csrf_token, discord_token_ciphertext, key_fingerprint, expires_at)
             VALUES ($1, 5, repeat('a', 43), '\\x00', $2, now() + $3::interval)",
        )
        .bind(&hash[..])
        .bind(&fingerprint[..])
        .bind(expires)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(
        "INSERT INTO jobs (kind, key, guild_id, max_attempts, dead_at)
         VALUES ('send_dm', 'send_dm:old', $1, 3, now() - interval '8 days'),
                ('send_dm', 'send_dm:new', $1, 3, now() - interval '1 day')",
    )
    .bind(i64::from(g))
    .execute(&mut *conn)
    .await?;
    let report = housekeeping::run(&mut conn, &[2; 8], Duration::from_secs(60)).await?;
    assert_eq!(
        (report.sessions, report.dead_jobs, report.more),
        (2, 1, false)
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(left, 1);
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn the_starter_rule_is_recorded_once(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = db(options).await?;
    let g = guild(&db, 1).await?;
    let (first, _) = rule(&db, g, "Spam").await?;
    let (second, _) = rule(&db, g, "Raids").await?;
    let mut conn = db.conn().await?;
    assert!(settings::set_starter_rule(&mut conn, g, first).await?);
    assert!(!settings::set_starter_rule(&mut conn, g, second).await?);
    Ok(())
}
