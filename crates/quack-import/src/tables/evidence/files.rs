//! `case_evidence_attachments` → `evidence_files`: reading a batch's
//! files and writing one, with the copy check.

use quack_core::case::MessageLink;
use uuid::Uuid;

use crate::ImportError;
use crate::convert::{self, Snowflake};
use crate::guild::Ctx;
use crate::source::Cols;
use crate::target::guarded;

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

/// One attachment as read.
pub(super) struct MySqlFile {
    pub(super) evidence: String,
    pub(super) key: String,
    id: Option<Uuid>,
    filename: String,
    content_type: Option<String>,
    size: i64,
    pub(super) url: String,
    pub(super) copy: Option<(i64, i64, i64)>,
    created_at: time::OffsetDateTime,
}

pub(super) async fn files_of(
    cx: &mut Ctx<'_>,
    owners: &[String],
) -> Result<Vec<MySqlFile>, ImportError> {
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

pub(super) async fn file(
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
