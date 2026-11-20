//! The messages around a saved message (§7.2 step 5): up to 5 before and 5
//! after a message staff added, text only, best effort. Failing to read
//! them never fails or delays the capture.

use std::time::Duration;

use quack_core::limits::{SURROUNDING_MESSAGES, SURROUNDING_TEXT_CHARS, truncate_chars};
use quack_db::evidence::{Evidence, SurroundingMessage};
use quack_db::jobs::ClaimedJob;

use crate::Engine;
use crate::port::{FetchedMessage, Outcome};

/// Two reads (10 s each) and a margin before the claim's deadline.
const ROOM: Duration = Duration::from_secs(25);

/// The messages around `item`'s message, oldest first; `None` when the item
/// was added by Quack (honeypot, import), time is short, or neither read
/// answered.
pub(super) async fn read(
    engine: &Engine,
    job: &ClaimedJob,
    item: &Evidence,
) -> Option<Vec<SurroundingMessage>> {
    item.added_by?;
    let (channel, message) = item.source()?;
    if job.time_left() < ROOM {
        return None;
    }
    let discord = engine.discord();
    let before = match discord
        .fetch_history(channel, Some(message), SURROUNDING_MESSAGES)
        .await
    {
        Outcome::Done(list) => Some(list),
        _ => None,
    };
    let after = match discord
        .fetch_history_after(channel, message, SURROUNDING_MESSAGES)
        .await
    {
        Outcome::Done(list) => Some(list),
        _ => None,
    };
    if before.is_none() && after.is_none() {
        return None;
    }
    let n = usize::from(SURROUNDING_MESSAGES);
    let mut before: Vec<FetchedMessage> = before
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.id < message && m.channel_id == channel)
        .collect();
    before.sort_by_key(|m| std::cmp::Reverse(m.id));
    before.truncate(n);
    let mut after: Vec<FetchedMessage> = after
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.id > message && m.channel_id == channel)
        .collect();
    after.sort_by_key(|m| m.id);
    after.truncate(n);
    let mut out: Vec<SurroundingMessage> = before
        .iter()
        .map(|m| saved(m, true))
        .chain(after.iter().map(|m| saved(m, false)))
        .collect();
    out.sort_by_key(|m| m.message_id);
    Some(out)
}

fn saved(m: &FetchedMessage, before: bool) -> SurroundingMessage {
    SurroundingMessage {
        message_id: m.id,
        before,
        author_id: m.author_id,
        content: (!m.content.trim().is_empty())
            .then(|| truncate_chars(&m.content, SURROUNDING_TEXT_CHARS).to_owned()),
        created_at: m.created_at,
        files: u32::try_from(m.attachments.len()).unwrap_or(u32::MAX),
    }
}
