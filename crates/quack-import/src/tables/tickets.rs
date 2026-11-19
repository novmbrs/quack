//! `tickets` → `tickets` (+ `ticket_post` messages and `ticket_closed`
//! DMs), `ticket_message_journal` → `ticket_messages`, `ticket_transcripts`
//! → `ticket_transcripts` (§19.4, §19.9).

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use time::OffsetDateTime;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::{Ctx, TicketInfo};
use crate::source::{Cols, Pages};
use crate::tables::publications::owned_id;
use crate::target::{self, DmState, NewDm, NewMessage, guarded};

/// A Go ticket's lifecycle in the new states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Life {
    Open,
    Closed(OffsetDateTime),
}

/// `status` → open or closed (§19.4); an open ticket with no thread, or a
/// second live ticket of the same member, is closed.
pub(crate) fn life(
    status: &str,
    resolved_at: Option<OffsetDateTime>,
    updated_at: OffsetDateTime,
) -> Option<Life> {
    match status {
        "open" => Some(Life::Open),
        "resolved" | "cancelled" => Some(Life::Closed(resolved_at.unwrap_or(updated_at))),
        _ => None,
    }
}

/// The close DM Go recorded under `metadata_json.close_notice`, so the
/// member is never sent it twice. Go never stored the DM channel, so a sent
/// notice is "couldn't confirm" rather than `sent`.
pub(crate) fn close_notice(metadata: &Value, at: OffsetDateTime) -> Option<DmState> {
    let notice = metadata.get("close_notice")?;
    Some(match notice.get("state").and_then(Value::as_str)? {
        "rejected" => DmState::Failed {
            failure: "discord_refused",
            at,
        },
        // `pending` was only ever looked for, never sent again; `sent` has
        // no DM channel to record.
        _ => DmState::Unknown { at },
    })
}

/// Imports the server's tickets with their journal and transcripts.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let extra_live = duplicate_live(cx).await?;
    let g = cx.g.ulid.clone();
    if cx.src.shape.has("tickets", "queue_delivery_attempt_id") {
        let rows = cx
            .src
            .rows(
                "SELECT id FROM tickets WHERE guild_id = ? AND queue_delivery_attempt_id IS NOT NULL AND queue_delivery_attempt_id <> ''",
                &[&g],
            )
            .await?;
        for row in &rows {
            let key = cx.key(Cols::new(row, "tickets", "id").key());
            cx.f.in_flight("ticket_queue_send_unconfirmed_dropped", key);
        }
    }
    let numbers = numbers(cx).await?;
    let mut pages = Pages::new(
        "SELECT * FROM tickets WHERE guild_id = ? AND id > ? ORDER BY id",
        "tickets",
        "id",
    );
    while let Some(rows) = pages.next(cx.src, &[&g]).await? {
        for row in &rows {
            let c = Cols::new(row, "tickets", "id");
            let extra = extra_live.contains(c.key());
            let number = numbers.get(c.key()).copied();
            ticket(cx, &c, extra, number).await?;
        }
    }
    journal(cx).await?;
    transcripts(cx).await
}

/// Each Go ticket's number in its server: 1, 2, … in creation order
/// (`created_at`, then id), §19.4. A skipped ticket leaves a gap.
async fn numbers(cx: &mut Ctx<'_>) -> Result<HashMap<String, i64>, ImportError> {
    let g = cx.g.ulid.clone();
    let rows = cx
        .src
        .rows(
            "SELECT id FROM tickets WHERE guild_id = ? ORDER BY created_at IS NULL, created_at, id",
            &[&g],
        )
        .await?;
    let mut out = HashMap::new();
    for (n, row) in (1_i64..).zip(&rows) {
        out.insert(Cols::new(row, "tickets", "id").key().to_owned(), n);
    }
    Ok(out)
}

/// Open tickets beyond the newest per member.
async fn duplicate_live(cx: &mut Ctx<'_>) -> Result<HashSet<String>, ImportError> {
    let g = cx.g.ulid.clone();
    let rows = cx
        .src
        .rows(
            "SELECT id, owner_discord_user_id FROM tickets WHERE guild_id = ? AND status = 'open'
             ORDER BY owner_discord_user_id, created_at DESC, id DESC",
            &[&g],
        )
        .await?;
    let mut out = HashSet::new();
    let mut last_owner: Option<String> = None;
    for row in &rows {
        let c = Cols::new(row, "tickets", "id");
        let owner = c.text("owner_discord_user_id")?;
        if last_owner.as_deref() == Some(owner.as_str()) {
            out.insert(c.key().to_owned());
        }
        last_owner = Some(owner);
    }
    Ok(out)
}

#[expect(
    clippy::too_many_lines,
    reason = "one ticket's mapping reads best top to bottom"
)]
async fn ticket(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    extra_live: bool,
    number: Option<i64>,
) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let Some(id) = c.uuid("id")? else {
        cx.f.skip("tickets", "invalid_id", &key);
        return Ok(());
    };
    let Snowflake::Id(member) = c.snowflake("owner_discord_user_id")? else {
        cx.f.skip("tickets", "invalid_snowflake", &key);
        return Ok(());
    };
    let updated = c.opt_time("updated_at")?;
    let created = c.opt_time("created_at")?;
    let resolved = c.opt_time("resolved_at")?;
    let created_at = created.or(updated).or(resolved).unwrap_or(cx.evaluated_at);
    let updated_at = updated.or(resolved).unwrap_or(created_at);
    if created.is_none() {
        cx.f.derived("ticket_created_at_missing", &key);
    }
    let thread = match c.snowflake("thread_discord_channel_id")? {
        Snowflake::Invalid => {
            cx.f.skip("tickets", "invalid_snowflake", format!("{key}:thread"));
            None
        }
        s => s.id(),
    };
    let status = c.text("status")?;
    let mut life = life(&status, resolved, updated_at).unwrap_or_else(|| {
        cx.f.derived("unknown_ticket_status_closed", &key);
        Life::Closed(updated_at)
    });
    if life == Life::Open && extra_live {
        cx.f.skip("tickets", "duplicate_live_ticket", &key);
        life = Life::Closed(updated_at);
    }
    if life == Life::Open && thread.is_none() {
        cx.f.derived("open_ticket_without_thread_closed", &key);
        life = Life::Closed(updated_at);
    }
    let log_channel = c.snowflake("log_channel_discord_id")?.id();
    // Without ticket settings the entry channel is unknown; the schema
    // allows that for a ticket that is not opening (imported ones never are).
    let entry = cx.ticket_entry_channel;
    if entry.is_none() {
        cx.f.derived("ticket_entry_channel_unknown", &key);
    }
    let closed = match life {
        Life::Open => None,
        Life::Closed(at) => Some(at),
    };
    let q = sqlx::query(
        "INSERT INTO tickets (id, guild_id, member_id, state, entry_channel_id, thread_id, opened_at, close_requested_at,
                              closed_at, closed_by, welcome_posted_at, frozen_at, transcript_saved_at, thread_deleted_at,
                              created_at, updated_at, number)
         VALUES ($1, $2, $3, $4::ticket_state, $5, $6, $7, $8, $8, $9, $7, $8, $8, $8, $10, $11, $12)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(member)
    .bind(if closed.is_some() { "closed" } else { "open" })
    .bind(entry)
    .bind(thread)
    .bind(created_at)
    .bind(closed)
    .bind(closed.and(c.snowflake("resolved_by_discord_user_id")?.id()))
    .bind(created_at)
    .bind(updated_at)
    // The trigger moves the server's ticket counter past it (§15.5).
    .bind(number);
    if !guarded(&mut cx.tx, &mut cx.f, "tickets", &key, q).await? {
        return Ok(());
    }
    cx.tickets.insert(id, TicketInfo { thread });
    if let (Some(channel), Some(message)) =
        (log_channel, c.snowflake("log_message_discord_id")?.id())
    {
        let m = NewMessage {
            id: owned_id(id, 3),
            kind: "ticket_post",
            case_id: None,
            appeal_id: None,
            ticket_id: Some(id),
            channel_id: Some(channel),
            message_id: Some(message),
            wanted: 1,
            posted: 1,
            created_at,
            updated_at,
        };
        target::message(&mut cx.tx, &mut cx.f, cx.g.id, &key, &m).await?;
    }
    if let Some(at) = closed
        && let Some(state) = close_notice(&c.json("metadata_json")?, at)
    {
        let dm = NewDm {
            id: owned_id(id, 4),
            member_id: member,
            kind: "ticket_closed",
            case_id: None,
            appeal_event_id: None,
            ticket_id: Some(id),
            state,
            created_at: at,
        };
        target::dm(&mut cx.tx, &mut cx.f, cx.g.id, &key, &dm).await?;
    }
    Ok(())
}

async fn journal(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    // The primary key is (guild, thread, message); page on the message
    // within this server (message IDs are unique across Discord).
    const SQL: &str =
        "SELECT * FROM ticket_message_journal WHERE guild_id = ? AND message_discord_id > ?
                       ORDER BY message_discord_id";
    let g = cx.g.ulid.clone();
    let mut pages = Pages::new(SQL, "ticket_message_journal", "message_discord_id");
    while let Some(rows) = pages.next(cx.src, &[&g]).await? {
        for row in &rows {
            let c = Cols::new(row, "ticket_message_journal", "message_discord_id");
            let key = cx.key(c.key());
            let Some(ticket) = c.uuid("ticket_id")? else {
                cx.f.skip("ticket_message_journal", "invalid_id", &key);
                continue;
            };
            let Some(info) = cx.tickets.get(&ticket).copied() else {
                cx.f.skip("ticket_message_journal", "missing_parent", &key);
                continue;
            };
            if info.thread.is_some()
                && c.snowflake("thread_discord_channel_id")?.id() != info.thread
            {
                cx.f.skip("ticket_message_journal", "thread_mismatch", &key);
                continue;
            }
            let (Snowflake::Id(message), Snowflake::Id(author)) = (
                c.snowflake("message_discord_id")?,
                c.snowflake("author_discord_user_id")?,
            ) else {
                cx.f.skip("ticket_message_journal", "invalid_snowflake", &key);
                continue;
            };
            let name = c
                .nonempty("author_name")?
                .unwrap_or_else(|| "unknown".to_owned());
            let (text, changed) = convert::clean(c.text("body")?);
            if changed {
                cx.f.normalized("ticket_messages.text", &key);
            }
            let q = sqlx::query(
                "INSERT INTO ticket_messages (guild_id, ticket_id, message_id, author_id, author_name, text, files, sent_at, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, '[]', $7, $8)",
            )
            .bind(cx.g.id)
            .bind(ticket)
            .bind(message)
            .bind(author)
            .bind(convert::clean(name).0)
            .bind(text)
            .bind(c.time("sent_at")?)
            .bind(c.opt_time("expires_at")?);
            guarded(&mut cx.tx, &mut cx.f, "ticket_messages", &key, q).await?;
        }
    }
    Ok(())
}

async fn transcripts(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        // Transcripts are large: one at a time keeps memory small.
        let rows = cx
            .src
            .rows(
                "SELECT * FROM ticket_transcripts WHERE guild_id = ? AND ticket_id > ? ORDER BY ticket_id LIMIT 1",
                &[&g, &after],
            )
            .await?;
        let Some(row) = rows.first() else { break };
        let c = Cols::new(row, "ticket_transcripts", "ticket_id");
        after = c.key().to_owned();
        let key = cx.key(c.key());
        let Some(ticket) = c.uuid("ticket_id")?.filter(|t| cx.tickets.contains_key(t)) else {
            cx.f.skip("ticket_transcripts", "missing_parent", &key);
            continue;
        };
        let expires = c.time("expires_at")?;
        if expires <= cx.evaluated_at {
            cx.f.skip("ticket_transcripts", "expired_transcript", &key);
            continue;
        }
        let (content, changed) = convert::clean(c.text("content")?);
        if changed {
            cx.f.normalized("ticket_transcripts.content", &key);
        }
        let q = sqlx::query(
            "INSERT INTO ticket_transcripts (ticket_id, guild_id, content, complete, partial_cause, captured_at, expires_at)
             VALUES ($1, $2, $3, false, 'history', $4, $5)",
        )
        .bind(ticket)
        .bind(cx.g.id)
        .bind(content)
        .bind(c.time("captured_at")?)
        .bind(expires);
        guarded(&mut cx.tx, &mut cx.f, "ticket_transcripts", &key, q).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lifecycles() {
        let t = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(life("open", None, t), Some(Life::Open));
        assert_eq!(life("resolved", None, t), Some(Life::Closed(t)));
        assert_eq!(life("cancelled", Some(t), t), Some(Life::Closed(t)));
        assert_eq!(life("weird", None, t), None);
    }

    #[test]
    fn close_notices_are_never_resent() {
        let t = OffsetDateTime::UNIX_EPOCH;
        for state in ["pending", "sent"] {
            let m = json!({"close_notice": {"state": state, "message_id": "5"}});
            assert_eq!(close_notice(&m, t), Some(DmState::Unknown { at: t }));
        }
        assert_eq!(
            close_notice(&json!({"close_notice": {"state": "rejected"}}), t),
            Some(DmState::Failed {
                failure: "discord_refused",
                at: t
            })
        );
        assert_eq!(close_notice(&json!({}), t), None);
    }
}
