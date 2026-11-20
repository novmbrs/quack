//! Which messages may become evidence (§7.1), checked before anything is
//! written: never a message in the evidence channel or one of Quack's copy
//! posts (they would show a staff-only or removed file as a saved message,
//! which the member always sees), and never a restore or a capture again of
//! an existing item by someone who can't read its channel now. The capture
//! job checks both again (§7.2), so a race only ends in a failed item.

use std::collections::HashSet;

use quack_core::case::{
    ContextProblem, ContextProblemCode, ContextValue, ContextValues, MessageLink,
};
use quack_core::ids::{AccountId, GuildId, MessageId};
use quack_core::rule::{Field, FieldKind};
use quack_db::DbError;

use super::access::{Unavailable, can_read};
use crate::Engine;

/// The messages among `links` that may never be evidence: any message in
/// the server's evidence channel, and Quack's copy posts wherever they are.
pub(crate) async fn evidence_copies(
    engine: &Engine,
    guild: GuildId,
    links: &[MessageLink],
) -> Result<HashSet<MessageId>, DbError> {
    if links.is_empty() {
        return Ok(HashSet::new());
    }
    let pairs: Vec<_> = links.iter().map(|l| (l.channel_id, l.message_id)).collect();
    let mut conn = engine.db().conn().await?;
    Ok(
        quack_db::evidence::evidence_copies(&mut conn, guild, &pairs)
            .await?
            .into_iter()
            .collect(),
    )
}

/// Whether `account` can read the channel of every link in `links` now;
/// the messages whose channel they can't read.
pub(crate) async fn unreadable(
    engine: &Engine,
    guild: GuildId,
    account: AccountId,
    links: &[&MessageLink],
) -> Result<HashSet<MessageId>, Unavailable> {
    let mut out = HashSet::new();
    for link in links {
        if !can_read(engine, guild, link.channel_id, account).await? {
            out.insert(link.message_id);
        }
    }
    Ok(out)
}

/// The `message_link` answers of `values` that point at a message that may
/// never be evidence, as context problems on their fields. `skip` holds
/// messages already on the case (they become no new evidence).
pub(crate) async fn context_copies(
    engine: &Engine,
    guild: GuildId,
    fields: &[Field],
    values: &ContextValues,
    skip: &HashSet<MessageId>,
) -> Result<Vec<ContextProblem>, DbError> {
    let answered: Vec<(String, MessageLink)> = fields
        .iter()
        .filter(|f| f.kind == FieldKind::MessageLink)
        .filter_map(|f| match values.get(&f.key) {
            Some(ContextValue::Text(url)) => MessageLink::parse_in(url, guild)
                .ok()
                .map(|l| (f.key.as_str().to_owned(), l)),
            _ => None,
        })
        .filter(|(_, l)| !skip.contains(&l.message_id))
        .collect();
    let links: Vec<MessageLink> = answered.iter().map(|(_, l)| *l).collect();
    let copies = evidence_copies(engine, guild, &links).await?;
    Ok(answered
        .into_iter()
        .filter(|(_, l)| copies.contains(&l.message_id))
        .map(|(key, _)| ContextProblem {
            key,
            code: ContextProblemCode::EvidenceChannel,
        })
        .collect())
}
