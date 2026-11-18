//! `appeal_notifications` → appeal update `dms` (audience `member`) and one
//! `appeal_post` tracked message per appeal (audience `staff`), §19.4, §19.9.

use std::collections::HashMap;

use uuid::Uuid;

use crate::ImportError;
use crate::convert::Snowflake;
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};
use crate::tables::notifications::{self, Notice};
use crate::tables::publications::owned_id;
use crate::target::{self, NewDm, NewMessage};

/// The newest staff post Go recorded for an appeal.
#[derive(Clone, Copy, Debug)]
struct Post {
    channel: Option<i64>,
    message: i64,
    refresh: bool,
}

/// Imports the appeal DMs, then one post per imported appeal.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let g = cx.g.ulid.clone();
    let mut posts: HashMap<Uuid, (time::OffsetDateTime, Post)> = HashMap::new();
    let mut after = String::new();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM appeal_notifications WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "appeal_notifications", "id")
            .key()
            .to_owned();
        for row in &rows {
            let c = Cols::new(row, "appeal_notifications", "id");
            match c.text("audience")?.as_str() {
                "member" => member_dm(cx, &c).await?,
                "staff" => staff_post(cx, &c, &mut posts)?,
                _ => {
                    cx.f.skip("appeal_notifications", "unknown_audience", cx.key(c.key()));
                }
            }
        }
        if rows.len() < BATCH {
            break;
        }
    }
    let mut appeals: Vec<Uuid> = cx.appeals.keys().copied().collect();
    appeals.sort();
    for appeal in appeals {
        post(cx, appeal, posts.get(&appeal).map(|(_, p)| *p)).await?;
    }
    Ok(())
}

async fn member_dm(cx: &mut Ctx<'_>, c: &Cols<'_>) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let (Some(id), Some(appeal), Some(event)) =
        (c.uuid("id")?, c.uuid("appeal_id")?, c.uuid("event_id")?)
    else {
        cx.f.skip("appeal_notifications", "invalid_id", &key);
        return Ok(());
    };
    let Some(info) = cx.appeals.get(&appeal).copied() else {
        cx.f.skip("appeal_notifications", "missing_parent", &key);
        return Ok(());
    };
    if cx.appeal_events.get(&event) != Some(&appeal) {
        cx.f.skip("appeal_notifications", "missing_parent", &key);
        return Ok(());
    }
    if c.snowflake("target_discord_user_id")? != Snowflake::Id(info.member) {
        cx.f.skip("appeal_notifications", "member_mismatch", &key);
        return Ok(());
    }
    let mut decided = notifications::decide(
        &Notice {
            status: &c.text("status")?,
            code: &c.text("last_error_code")?,
            channel: c.snowflake("delivery_channel_id")?.id(),
            message: c.snowflake("delivery_message_id")?.id(),
            sent_at: None,
            updated_at: c.time("updated_at")?,
        },
        false,
    );
    decided.in_flight = decided.in_flight.map(|row| match row {
        "dm_pending_resumed" => "appeal_dm_pending_resumed",
        "dm_sending_unknown" => "appeal_dm_sending_unknown",
        other => other,
    });
    notifications::record(cx, &key, &decided);
    let dm = NewDm {
        id,
        member_id: info.member,
        kind: "appeal_update",
        case_id: Some(info.case),
        appeal_event_id: Some(event),
        ticket_id: None,
        state: decided.state,
        created_at: c.time("created_at")?,
    };
    target::dm(&mut cx.tx, &mut cx.f, cx.g.id, &key, &dm).await?;
    Ok(())
}

/// Keeps the newest staff row with a message per appeal; the rest are
/// counted (their bodies and leases are not imported).
fn staff_post(
    cx: &mut Ctx<'_>,
    c: &Cols<'_>,
    posts: &mut HashMap<Uuid, (time::OffsetDateTime, Post)>,
) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let Some(appeal) = c.uuid("appeal_id")? else {
        cx.f.skip("appeal_notifications", "invalid_id", &key);
        return Ok(());
    };
    let Snowflake::Id(message) = c.snowflake("delivery_message_id")? else {
        // A staff post Go never delivered: the appeal's own post replaces it.
        let status = c.text("status")?;
        let undelivered = matches!(status.as_str(), "pending" | "claimed" | "sending")
            || (status == "failed" && c.text("last_error_code")? == "delivery_deferred");
        if undelivered {
            cx.f.in_flight("appeal_staff_post_never_delivered", &key);
        } else {
            cx.f.skip("appeal_notifications", "staff_row_without_message", &key);
        }
        return Ok(());
    };
    let at = c.time("created_at")?;
    let post = Post {
        channel: c.snowflake("delivery_channel_id")?.id(),
        message,
        refresh: c.flag("refresh_requested")?,
    };
    match posts.get(&appeal) {
        Some((seen, _)) if *seen > at => {
            cx.f.skip("appeal_notifications", "older_staff_post", &key);
        }
        _ => {
            posts.insert(appeal, (at, post));
        }
    }
    Ok(())
}

/// The appeal's tracked staff post (§19.4): the newest Go post, re-rendered
/// for open appeals so the new buttons replace the old; otherwise a post
/// waiting for (or, for an open appeal, created in) the appeal channel.
async fn post(cx: &mut Ctx<'_>, appeal: Uuid, found: Option<Post>) -> Result<(), ImportError> {
    let Some(info) = cx.appeals.get(&appeal).copied() else {
        return Ok(());
    };
    let key = cx.key(&appeal.to_string());
    let found = found.or_else(|| {
        info.review_message.map(|message| Post {
            channel: None,
            message,
            refresh: false,
        })
    });
    let (channel, message, wanted, posted) = match found {
        Some(p) => {
            let channel = p.channel.or_else(|| {
                cx.f.derived("appeal_post_channel_from_settings", &key);
                cx.appeal_channel
            });
            match channel {
                Some(channel) if info.open || p.refresh => {
                    cx.f.in_flight(
                        if p.refresh {
                            "appeal_post_refresh_rerendered"
                        } else {
                            "appeal_post_rerendered"
                        },
                        &key,
                    );
                    (Some(channel), Some(p.message), 2, 1)
                }
                Some(channel) => (Some(channel), Some(p.message), 1, 1),
                None => (None, None, 1, 0),
            }
        }
        // The Go bot never posted it: the row waits without a channel, so
        // the import never creates a staff post Go didn't make. Saving the
        // appeal channel after cutover binds it (§8.3).
        None if info.open => {
            cx.f.in_flight("appeal_post_unposted", &key);
            (None, None, 1, 0)
        }
        None => (None, None, 1, 0),
    };
    let m = NewMessage {
        id: owned_id(appeal, 2),
        kind: "appeal_post",
        case_id: None,
        appeal_id: Some(appeal),
        ticket_id: None,
        channel_id: channel,
        message_id: message,
        wanted,
        posted,
        created_at: info.created_at,
        updated_at: info.updated_at,
    };
    target::message(&mut cx.tx, &mut cx.f, cx.g.id, &key, &m).await?;
    Ok(())
}
