//! `case_publications` → `messages` of kind `case_receipt` (§19.4): one per
//! case, the latest publication with a message ID, already up to date
//! (`1`/`1`): old receipts are not re-rendered.

use std::collections::HashSet;

use uuid::Uuid;

use crate::ImportError;
use crate::convert::Snowflake;
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};
use crate::target::{self, NewMessage};

/// Imports the server's case receipts, a page of cases at a time.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        let cases = cx
            .src
            .page(
                "SELECT DISTINCT p.case_id FROM case_publications p JOIN cases c ON c.id = p.case_id
                 WHERE c.guild_id = ? AND p.case_id > ? ORDER BY p.case_id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = cases.last() else { break };
        after = Cols::new(last, "case_publications", "case_id")
            .key()
            .to_owned();
        let ids: Vec<String> = cases
            .iter()
            .map(|r| {
                Cols::new(r, "case_publications", "case_id")
                    .key()
                    .to_owned()
            })
            .collect();
        let sql = format!(
            "SELECT * FROM case_publications WHERE case_id IN ({}) ORDER BY case_id, created_at DESC, message_id DESC",
            vec!["?"; ids.len()].join(", ")
        );
        let binds: Vec<&str> = ids.iter().map(String::as_str).collect();
        let rows = cx.src.rows(&sql, &binds).await?;
        // The latest publication per case wins; earlier ones are reported.
        let mut seen: HashSet<String> = HashSet::new();
        for row in &rows {
            let c = Cols::new(row, "case_publications", "message_id");
            if !seen.insert(c.text("case_id")?) {
                let key = cx.key(c.key());
                cx.f.skip("case_publications", "older_publication", key);
                continue;
            }
            one(cx, &c).await?;
        }
        if cases.len() < BATCH {
            break;
        }
    }
    Ok(())
}

async fn one(cx: &mut Ctx<'_>, c: &Cols<'_>) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let Some(case_id) = c.uuid("case_id")? else {
        cx.f.skip("case_publications", "invalid_id", &key);
        return Ok(());
    };
    if !cx.cases.contains_key(&case_id) {
        cx.f.skip("case_publications", "missing_parent", &key);
        return Ok(());
    }
    let (Snowflake::Id(message), Snowflake::Id(channel)) =
        (c.snowflake("message_id")?, c.snowflake("channel_id")?)
    else {
        cx.f.skip("case_publications", "invalid_snowflake", &key);
        return Ok(());
    };
    // The presentation names its server when it names one; it must be this one.
    let presentation = c.json_text("presentation_json").unwrap_or_else(|_| {
        cx.f.derived("invalid_presentation_json", &key);
        None
    });
    if let Some(guild) = presentation
        .as_ref()
        .and_then(|p| p.get("discord_guild_id"))
        .and_then(|v| v.as_str())
        && guild != cx.g.id.to_string()
    {
        cx.f.skip("case_publications", "cross_guild", &key);
        return Ok(());
    }
    if c.flag("refresh_requested")? {
        cx.f.in_flight("receipt_refresh_not_rerendered", &key);
    }
    let m = NewMessage {
        id: receipt_row_id(case_id),
        kind: "case_receipt",
        case_id: Some(case_id),
        appeal_id: None,
        ticket_id: None,
        channel_id: Some(channel),
        message_id: Some(message),
        wanted: 1,
        posted: 1,
        created_at: c.time("created_at")?,
        updated_at: c.time("updated_at")?,
    };
    target::message(&mut cx.tx, &mut cx.f, cx.g.id, &key, &m).await?;
    Ok(())
}

/// A tracked message has no Go ID of its own; its ID derives from its
/// owner's so a rerun writes the same row.
pub(crate) fn receipt_row_id(owner: Uuid) -> Uuid {
    owned_id(owner, 1)
}

/// A deterministic UUID for a row owned by `owner`, distinct per `slot`.
pub(crate) fn owned_id(owner: Uuid, slot: u8) -> Uuid {
    let mut bytes = *owner.as_bytes();
    // Keep the timestamp, version, and variant; mark the slot in the last
    // byte so a case's receipt and an appeal's post never collide.
    bytes[15] ^= 0xA0 | slot;
    Uuid::from_bytes(bytes)
}

/// A deterministic UUID for a server-level tracked message (`slot` names
/// which: ticket panel, honeypot warning).
pub(crate) fn guild_row_id(guild: i64, slot: u8) -> Uuid {
    owned_id(
        Uuid::from_u128(u128::from(guild.unsigned_abs()) << 64),
        slot,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_ids_differ_per_slot_and_are_stable() {
        let owner = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
        assert_ne!(owned_id(owner, 1), owned_id(owner, 2));
        assert_ne!(owned_id(owner, 1), owner);
        assert_eq!(owned_id(owner, 1), receipt_row_id(owner));
        assert_ne!(guild_row_id(5, 1), guild_row_id(6, 1));
    }
}
