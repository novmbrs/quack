//! `case_evidence_snapshots` → `evidence`, `case_evidence_attachments` →
//! `evidence_files` (§19.4). Duplicate `(case, message)` snapshots merge
//! into the earliest, files deduplicated by attachment ID.

use std::collections::{HashMap, HashSet};

use quack_core::case::MessageLink;
use quack_core::limits;
use quack_db::evidence::EmbedSummary;
use serde_json::Value;
use uuid::Uuid;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::source::{BATCH, Cols};
use crate::target::guarded;

/// `capture_outcome` → `(state, failure)`.
pub(crate) fn outcome(raw: &str) -> Option<(&'static str, Option<&'static str>)> {
    Some(match raw {
        "captured" | "uploaded" => ("saved", None),
        "deleted" => ("failed", Some("message_deleted")),
        "inaccessible" => ("failed", Some("quack_no_access")),
        "unavailable" => ("failed", Some("discord_error")),
        _ => return None,
    })
}

/// Go embeds → `{type, title, description, url}` summaries (at most 10),
/// and how many were left out.
pub(crate) fn embeds(raw: &Value) -> (Vec<EmbedSummary>, usize) {
    let items = raw.as_array().map_or(&[][..], Vec::as_slice);
    let text = |e: &Value, k: &str, max: usize| {
        e.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| convert::truncate(&convert::clean(s.to_owned()).0, max))
    };
    let out: Vec<EmbedSummary> = items
        .iter()
        .take(limits::MAX_EVIDENCE_EMBEDS)
        .map(|e| EmbedSummary {
            kind: text(e, "type", 32).unwrap_or_else(|| "rich".to_owned()),
            title: text(e, "title", limits::EVIDENCE_EMBED_TITLE_CHARS),
            description: text(e, "description", limits::EVIDENCE_EMBED_DESCRIPTION_CHARS),
            url: text(e, "url", 2048),
        })
        .collect();
    (out, items.len().saturating_sub(limits::MAX_EVIDENCE_EMBEDS))
}

/// A file's copy columns `(channel, message, attachment)`: `copied` only
/// when the preserved URL agrees with the preserved ID columns. Go wrote the
/// copy's jump link (`…/channels/<server>/<channel>/<message>`, which must
/// name this server and the preserved message); a CDN URL
/// (`…/attachments/<channel>/<attachment>/…`) must name the preserved
/// attachment.
pub(crate) fn copy(
    outcome: &str,
    url: &str,
    guild: i64,
    message: Snowflake,
    attachment: Snowflake,
) -> Option<(i64, i64, i64)> {
    if outcome != "preserved" {
        return None;
    }
    let (Snowflake::Id(message), Snowflake::Id(attachment)) = (message, attachment) else {
        return None;
    };
    if let Ok(link) = MessageLink::parse(url) {
        return (link.guild_id.to_i64() == guild && link.message_id.to_i64() == message)
            .then_some((link.channel_id.to_i64(), message, attachment));
    }
    let (channel, from_url) = convert::attachment_parts(url)?;
    (from_url == attachment).then_some((channel, message, attachment))
}

/// Imports the server's evidence, batch by batch with their files.
pub(crate) async fn import(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    let dupes = duplicates(cx).await?;
    let g = cx.g.ulid.clone();
    let mut after = String::new();
    loop {
        let rows = cx
            .src
            .page(
                "SELECT * FROM case_evidence_snapshots WHERE guild_id = ? AND id > ? ORDER BY id",
                &[&g],
                &after,
            )
            .await?;
        let Some(last) = rows.last() else { break };
        after = Cols::new(last, "case_evidence_snapshots", "id")
            .key()
            .to_owned();
        let mut owners: Vec<String> = Vec::new();
        for row in &rows {
            let key = Cols::new(row, "case_evidence_snapshots", "id")
                .key()
                .to_owned();
            if let Some(merged) = dupes.merged.get(&key) {
                owners.extend(merged.iter().cloned());
            }
            owners.push(key);
        }
        let files = files_of(cx, &owners).await?;
        for row in &rows {
            let c = Cols::new(row, "case_evidence_snapshots", "id");
            if dupes.losers.contains(c.key()) {
                let key = cx.key(c.key());
                cx.f.skip("case_evidence_snapshots", "duplicate_evidence", key);
                continue;
            }
            let mut mine: Vec<&MySqlFile> =
                files.iter().filter(|f| f.evidence == c.key()).collect();
            if let Some(merged) = dupes.merged.get(c.key()) {
                mine.extend(files.iter().filter(|f| merged.contains(&f.evidence)));
            }
            snapshot(cx, &c, &mine).await?;
        }
        if rows.len() < BATCH {
            break;
        }
    }
    Ok(())
}

struct Dupes {
    /// Later duplicates, skipped.
    losers: HashSet<String>,
    /// Kept snapshot → the duplicates merged into it.
    merged: HashMap<String, Vec<String>>,
}

async fn duplicates(cx: &mut Ctx<'_>) -> Result<Dupes, ImportError> {
    let g = cx.g.ulid.clone();
    let rows = cx
        .src
        .rows(
            "SELECT s.id, s.case_id, s.message_discord_id FROM case_evidence_snapshots s
             JOIN (SELECT case_id, message_discord_id FROM case_evidence_snapshots WHERE guild_id = ?
                   GROUP BY case_id, message_discord_id HAVING COUNT(*) > 1) d
               ON d.case_id = s.case_id AND d.message_discord_id = s.message_discord_id
             WHERE s.guild_id = ? ORDER BY s.case_id, s.message_discord_id, s.created_at, s.id",
            &[&g, &g],
        )
        .await?;
    let mut out = Dupes {
        losers: HashSet::new(),
        merged: HashMap::new(),
    };
    let mut current: Option<(String, String, String)> = None;
    for row in &rows {
        let c = Cols::new(row, "case_evidence_snapshots", "id");
        let group = (c.text("case_id")?, c.text("message_discord_id")?);
        match &current {
            Some((case, message, keeper)) if *case == group.0 && *message == group.1 => {
                out.losers.insert(c.key().to_owned());
                out.merged
                    .entry(keeper.clone())
                    .or_default()
                    .push(c.key().to_owned());
            }
            _ => current = Some((group.0, group.1, c.key().to_owned())),
        }
    }
    Ok(out)
}

/// One attachment as read.
struct MySqlFile {
    evidence: String,
    key: String,
    id: Option<Uuid>,
    filename: String,
    content_type: Option<String>,
    size: i64,
    url: String,
    copy: Option<(i64, i64, i64)>,
    created_at: time::OffsetDateTime,
}

async fn files_of(cx: &mut Ctx<'_>, owners: &[String]) -> Result<Vec<MySqlFile>, ImportError> {
    if owners.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT * FROM case_evidence_attachments WHERE evidence_id IN ({}) ORDER BY created_at, id",
        vec!["?"; owners.len()].join(", ")
    );
    let binds: Vec<&str> = owners.iter().map(String::as_str).collect();
    let rows = cx.src.rows(&sql, &binds).await?;
    let mut out = Vec::new();
    for row in &rows {
        let c = Cols::new(row, "case_evidence_attachments", "id");
        let url = c.text("original_url")?;
        let outcome = c.text("copy_outcome")?;
        let copy = copy(
            &outcome,
            &c.text("preserved_url")?,
            cx.g.id,
            c.snowflake("preserved_message_discord_id")?,
            c.snowflake("preserved_attachment_discord_id")?,
        );
        if outcome == "preserved" && copy.is_none() {
            cx.f.derived("copy_unavailable", cx.key(c.key()));
        }
        out.push(MySqlFile {
            evidence: c.text("evidence_id")?,
            key: c.key().to_owned(),
            id: c.uuid("id")?,
            filename: c.text("filename")?,
            content_type: c.nonempty("content_type")?,
            size: c.int("size_bytes")?,
            url,
            copy,
            created_at: c.time("created_at")?,
        });
    }
    Ok(out)
}

#[expect(
    clippy::too_many_lines,
    reason = "one snapshot's columns, read top to bottom"
)]
async fn snapshot(cx: &mut Ctx<'_>, c: &Cols<'_>, files: &[&MySqlFile]) -> Result<(), ImportError> {
    let key = cx.key(c.key());
    let skip_files = |cx: &mut Ctx<'_>, files: &[&MySqlFile]| {
        for f in files {
            cx.f.skip(
                "case_evidence_attachments",
                "missing_parent",
                cx.key(&f.key),
            );
        }
    };
    let (Some(id), Some(case_id)) = (c.uuid("id")?, c.uuid("case_id")?) else {
        cx.f.skip("case_evidence_snapshots", "invalid_id", &key);
        skip_files(cx, files);
        return Ok(());
    };
    if !cx.cases.contains_key(&case_id) {
        cx.f.skip("case_evidence_snapshots", "missing_parent", &key);
        skip_files(cx, files);
        return Ok(());
    }
    let raw_outcome = c.text("capture_outcome")?;
    let coordinates = match (
        c.snowflake("channel_discord_id")?,
        c.snowflake("message_discord_id")?,
    ) {
        (Snowflake::Id(channel), Snowflake::Id(message)) => Some((channel, message)),
        // A file a moderator uploaded has no source message: it is kept as
        // evidence of its copy's message in the evidence channel.
        _ if raw_outcome == "uploaded" => {
            files
                .iter()
                .find_map(|f| f.copy)
                .map(|(channel, message, _)| {
                    cx.f.derived("uploaded_file_as_copy_message", &key);
                    (channel, message)
                })
        }
        _ => None,
    };
    let Some((channel, message)) = coordinates else {
        cx.f.skip("case_evidence_snapshots", "invalid_snowflake", &key);
        skip_files(cx, files);
        return Ok(());
    };
    let (state, failure) = outcome(&raw_outcome).unwrap_or_else(|| {
        cx.f.derived("unknown_capture_outcome", &key);
        ("failed", Some("discord_error"))
    });
    let content = c.nonempty("content")?.map(|t| {
        let (t, changed) = convert::clean(t);
        if changed {
            cx.f.normalized("evidence.content", &key);
        }
        if convert::chars(&t) > limits::EVIDENCE_TEXT_CHARS {
            // The column's CHECK holds 4,000 characters, Go's own cap.
            cx.f.over_limit("evidence.content", &key);
            convert::truncate(&t, limits::EVIDENCE_TEXT_CHARS)
        } else {
            t
        }
    });
    let message_time =
        |cx: &mut Ctx<'_>, column: &str| -> Result<Option<time::OffsetDateTime>, ImportError> {
            let raw = c.naive(column)?;
            let at = raw.and_then(convert::message_time);
            if raw.is_some() && at.is_none() {
                cx.f.derived("zero_message_time", format!("{key}:{column}"));
            }
            Ok(at)
        };
    let created = message_time(cx, "message_created_at")?;
    let edited = message_time(cx, "message_edited_at")?;
    let (summaries, extra) = embeds(&c.json("embeds_json")?);
    if extra > 0 {
        cx.f.derived("embeds_not_kept", &key);
    }
    let embeds =
        serde_json::to_value(&summaries).map_err(|e| ImportError::server(e.to_string()))?;
    let mut kept: Vec<&MySqlFile> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for f in files {
        let attachment =
            convert::attachment_parts(&f.url).map_or_else(|| f.key.clone(), |(_, a)| a.to_string());
        if !seen.insert(attachment) {
            cx.f.skip(
                "case_evidence_attachments",
                "duplicate_evidence",
                cx.key(&f.key),
            );
            continue;
        }
        kept.push(f);
    }
    let not_saved = kept.len().saturating_sub(10);
    let updated = c.time("updated_at")?;
    let q = sqlx::query(
        "INSERT INTO evidence (id, guild_id, case_id, channel_id, message_id, added_by, state, failure, author_id, content,
                               message_created_at, message_edited_at, embeds, files_not_saved, created_at, saved_at)
         VALUES ($1, $2, $3, $4, $5, NULL, $6::evidence_state, $7::evidence_failure, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(case_id)
    .bind(channel)
    .bind(message)
    .bind(state)
    .bind(failure)
    .bind(c.snowflake("author_discord_user_id")?.id())
    .bind(content)
    .bind(created)
    .bind(edited)
    .bind(embeds)
    .bind(i32::try_from(not_saved).unwrap_or(i32::MAX))
    .bind(c.time("created_at")?)
    .bind((state == "saved").then_some(updated));
    if !guarded(&mut cx.tx, &mut cx.f, "evidence", &key, q).await? {
        skip_files(cx, files);
        return Ok(());
    }
    for (i, f) in kept.iter().enumerate() {
        if i >= 10 {
            cx.f.skip(
                "case_evidence_attachments",
                "extra_attachment",
                cx.key(&f.key),
            );
            continue;
        }
        file(cx, id, case_id, i16::try_from(i + 1).unwrap_or(10), f).await?;
    }
    Ok(())
}

async fn file(
    cx: &mut Ctx<'_>,
    evidence: Uuid,
    case_id: Uuid,
    position: i16,
    f: &MySqlFile,
) -> Result<(), ImportError> {
    let key = cx.key(&f.key);
    let Some(id) = f.id else {
        cx.f.skip("case_evidence_attachments", "invalid_id", &key);
        return Ok(());
    };
    let mut filename = convert::clean(f.filename.trim().to_owned()).0;
    if filename.is_empty() {
        "file".clone_into(&mut filename);
        cx.f.derived("empty_filename", &key);
    }
    let size = if f.size < 0 {
        cx.f.normalized("evidence_files.size_bytes", &key);
        0
    } else {
        f.size
    };
    let (state, (cc, cm, ca)) = match f.copy {
        Some((c, m, a)) => ("copied", (Some(c), Some(m), Some(a))),
        None => ("link_only", (None, None, None)),
    };
    let q = sqlx::query(
        "INSERT INTO evidence_files (id, guild_id, case_id, evidence_id, position, attachment_id, filename, content_type,
                                     size_bytes, original_url, copy_state, copy_channel_id, copy_message_id,
                                     copy_attachment_id, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::file_copy_state, $12, $13, $14, $15)",
    )
    .bind(id)
    .bind(cx.g.id)
    .bind(case_id)
    .bind(evidence)
    .bind(position)
    .bind(convert::attachment_parts(&f.url).map(|(_, a)| a))
    .bind(filename)
    .bind(&f.content_type)
    .bind(size)
    .bind(&f.url)
    .bind(state)
    .bind(cc)
    .bind(cm)
    .bind(ca)
    .bind(f.created_at);
    guarded(&mut cx.tx, &mut cx.f, "evidence_files", &key, q).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outcomes() {
        assert_eq!(outcome("uploaded"), Some(("saved", None)));
        assert_eq!(
            outcome("deleted"),
            Some(("failed", Some("message_deleted")))
        );
        assert_eq!(outcome("other"), None);
    }

    #[test]
    fn embed_summaries() {
        let many: Vec<Value> = (0..12)
            .map(|i| json!({"type": "link", "title": format!("t{i}")}))
            .collect();
        let (s, extra) = embeds(&Value::Array(many));
        assert_eq!((s.len(), extra), (10, 2));
        let (s, _) = embeds(&json!([{"description": "d", "url": "u"}]));
        assert_eq!(s[0].kind, "rich");
        assert_eq!(s[0].title, None);
        assert_eq!(embeds(&json!(null)).0.len(), 0);
    }

    #[test]
    fn copies_must_agree() {
        let url = "https://cdn.discordapp.com/attachments/5/7/a.png";
        let link = "https://discord.com/channels/9/5/6";
        assert_eq!(
            copy("preserved", link, 9, Snowflake::Id(6), Snowflake::Id(7)),
            Some((5, 6, 7))
        );
        assert_eq!(
            copy("preserved", link, 8, Snowflake::Id(6), Snowflake::Id(7)),
            None
        );
        assert_eq!(
            copy("preserved", link, 9, Snowflake::Id(4), Snowflake::Id(7)),
            None
        );
        assert_eq!(
            copy("preserved", url, 9, Snowflake::Id(6), Snowflake::Id(7)),
            Some((5, 6, 7))
        );
        assert_eq!(
            copy("preserved", url, 9, Snowflake::Id(6), Snowflake::Id(8)),
            None
        );
        assert_eq!(
            copy("preserved", "", 9, Snowflake::Id(6), Snowflake::Id(7)),
            None
        );
        assert_eq!(
            copy("metadata_only", url, 9, Snowflake::Id(6), Snowflake::Id(7)),
            None
        );
    }
}
