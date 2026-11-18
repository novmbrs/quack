//! Lock-order and tenant regressions (from the quack-db review).

#![allow(clippy::unwrap_used)]

use anyhow::Result;
use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId, TicketId};
use quack_core::modules::ActivityEvent;
use quack_db::{Db, DbError, activity_log, tickets};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;

async fn seed(db: &Db) -> Result<GuildId> {
    let mut conn = db.conn().await?;
    sqlx::query("INSERT INTO guilds (guild_id,name,owner_id) VALUES (1,'Test',1)")
        .execute(&mut *conn)
        .await?;
    Ok(GuildId::new(1).unwrap())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn activity_route_delete_and_replace_deadlock(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let g = seed(&db).await?;
    let c = ChannelId::new(11).unwrap();
    let by = AccountId::new(1).unwrap();
    let mut replace = db.begin().await?;
    sqlx::raw_sql(
        "INSERT INTO activity_log_settings(guild_id,enabled) VALUES (1,true);
        INSERT INTO activity_log_routes(guild_id,event,channel_id) VALUES (1,'message_deleted',11)",
    )
    .execute(&mut *replace)
    .await?;
    replace.commit().await?;
    let mut replace = db.begin().await?;
    activity_log::settings(&mut replace, g, true).await?;
    let mut remove = db.begin().await?;
    let mut observer = db.conn().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *remove)
        .await?;
    let deleting = tokio::spawn(async move {
        let result = activity_log::remove_channel(&mut remove, g, c).await;
        remove.rollback().await.unwrap();
        result
    });
    tokio::time::timeout(Duration::from_secs(4), async {
        while !sqlx::query_scalar::<_, bool>("SELECT cardinality(pg_blocking_pids($1))>0")
            .bind(pid)
            .fetch_one(&mut *observer)
            .await?
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await??;
    let setting = activity_log::update_settings(
        &mut replace,
        g,
        activity_log::ActivityFlags {
            include_message_content: false,
            include_file_names: false,
            include_embed_types: false,
        },
        &[(ActivityEvent::MessageDeleted, c)],
        1,
        by,
    )
    .await;
    replace.rollback().await?;
    let deleting = deleting.await?;
    let deadlock = [setting.err(), deleting.err()]
        .into_iter()
        .flatten()
        .any(|e| matches!(e, DbError::Contention(..)));
    assert!(
        !deadlock,
        "PostgreSQL detected the settings/route lock inversion"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn journal_does_not_admit_after_freeze(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let g = seed(&db).await?;
    let ticket = TicketId::from_uuid(uuid::Uuid::now_v7());
    let mut freeze = db.begin().await?;
    sqlx::query("INSERT INTO tickets(id,guild_id,member_id,state,entry_channel_id) VALUES ($1,1,9,'closing',11)")
        .bind(ticket.get()).execute(&mut *freeze).await?;
    freeze.commit().await?;
    let mut freeze = db.begin().await?;
    assert!(tickets::mark_frozen(&mut freeze, g, ticket).await?);
    let mut journal = db.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *journal)
        .await?;
    let (send_added, mut recv_added) = tokio::sync::oneshot::channel();
    let (send_commit, recv_commit) = tokio::sync::oneshot::channel();
    let insertion = tokio::spawn(async move {
        let added = tickets::add_journal_message(
            &mut journal,
            g,
            ticket,
            &tickets::JournalMessage {
                message: MessageId::new(123).unwrap(),
                author: AccountId::new(9).unwrap(),
                author_name: "Member".to_owned(),
                text: "late row".to_owned(),
                files: vec![],
                sent_at: time::OffsetDateTime::now_utc(),
            },
        )
        .await?;
        let _ = send_added.send(added);
        let _ = recv_commit.await;
        journal.commit().await?;
        Ok::<bool, DbError>(added)
    });
    let mut reader = db.conn().await?;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let blocked =
                sqlx::query_scalar::<_, bool>("SELECT cardinality(pg_blocking_pids($1)) > 0")
                    .bind(pid)
                    .fetch_one(&mut *reader)
                    .await?;
            if blocked || recv_added.try_recv().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok::<_, sqlx::Error>(())
    })
    .await??;
    freeze.commit().await?;
    assert_eq!(tickets::journal(&mut reader, g, ticket).await?.len(), 0);
    let _ = send_commit.send(());
    let added = insertion.await??;
    assert!(
        !added,
        "journal row admitted after mark_frozen and could commit after transcript read"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn evidence_conflict_does_not_cross_guilds(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    use quack_core::ids::{CaseId, EvidenceId};
    use quack_db::evidence;
    let db = Db::connect_with(options, 4).await?;
    let _ = seed(&db).await?;
    let mut conn = db.conn().await?;
    let case = CaseId::from_uuid(uuid::Uuid::from_u128(1));
    let item = EvidenceId::from_uuid(uuid::Uuid::now_v7());
    sqlx::raw_sql(
        "INSERT INTO guilds(guild_id,name,owner_id) VALUES (2,'Second',2);
        INSERT INTO cases(id,guild_id,number,member_id,source,sends_dm,imported)
        VALUES ('00000000-0000-0000-0000-000000000001',1,1,9,'imported',false,'{}');
        INSERT INTO evidence(guild_id,case_id,channel_id,message_id,state,failure)
        VALUES (1,'00000000-0000-0000-0000-000000000001',11,12,'failed','discord_error');",
    )
    .execute(&mut *conn)
    .await?;
    let outcome = evidence::admit(
        &mut conn,
        GuildId::new(2).unwrap(),
        case,
        item,
        ChannelId::new(11).unwrap(),
        MessageId::new(12).unwrap(),
        None,
    )
    .await;
    let state: String = sqlx::query_scalar("SELECT state::text FROM evidence")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(
        state, "failed",
        "wrong-guild admission returned {outcome:?}"
    );
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn removing_one_of_several_routes_advances_the_version(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let g = seed(&db).await?;
    let mut conn = db.conn().await?;
    sqlx::raw_sql(
        "INSERT INTO activity_log_settings(guild_id,enabled) VALUES (1,true);
        INSERT INTO activity_log_routes(guild_id,event,channel_id)
        VALUES (1,'message_deleted',11), (1,'message_edited',12)",
    )
    .execute(&mut *conn)
    .await?;
    drop(conn);
    let before = {
        let mut tx = db.begin().await?;
        let s = activity_log::settings(&mut tx, g, false).await?.unwrap();
        tx.rollback().await?;
        s.version
    };
    let mut tx = db.begin().await?;
    let (removed, off) =
        activity_log::remove_channel(&mut tx, g, ChannelId::new(11).unwrap()).await?;
    tx.commit().await?;
    assert_eq!((removed, off), (1, false));
    let mut tx = db.begin().await?;
    let stale = activity_log::update_settings(
        &mut tx,
        g,
        activity_log::ActivityFlags {
            include_message_content: false,
            include_file_names: false,
            include_embed_types: false,
        },
        &[
            (ActivityEvent::MessageDeleted, ChannelId::new(11).unwrap()),
            (ActivityEvent::MessageEdited, ChannelId::new(12).unwrap()),
        ],
        before,
        AccountId::new(1).unwrap(),
    )
    .await?;
    tx.rollback().await?;
    assert_eq!(stale, None, "an edit against the old routes is refused");
    Ok(())
}

#[sqlx::test(migrator = "quack_db::MIGRATOR")]
async fn ticket_steps_refuse_the_wrong_state(
    _: PgPoolOptions,
    options: PgConnectOptions,
) -> Result<()> {
    let db = Db::connect_with(options, 4).await?;
    let g = seed(&db).await?;
    let ticket = TicketId::from_uuid(uuid::Uuid::now_v7());
    let mut conn = db.conn().await?;
    sqlx::query("INSERT INTO tickets(id,guild_id,member_id,state,entry_channel_id,thread_id,opened_at) VALUES ($1,1,9,'open',11,21,now())")
        .bind(ticket.get())
        .execute(&mut *conn)
        .await?;
    assert!(
        !tickets::save_transcript(&mut conn, g, ticket, "early", None, 30).await?,
        "no transcript for an open ticket"
    );
    assert!(
        !tickets::mark_thread_deleted(&mut conn, g, ticket, false).await?,
        "an open ticket's thread is not recorded as deleted"
    );
    // After retention deleted the transcript, it is not saved again.
    sqlx::query("UPDATE tickets SET state='closing', frozen_at=now(), transcript_saved_at=now() WHERE id=$1")
        .bind(ticket.get())
        .execute(&mut *conn)
        .await?;
    assert!(!tickets::save_transcript(&mut conn, g, ticket, "again", None, 30).await?);
    Ok(())
}
