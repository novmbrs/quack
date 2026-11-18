//! The moderation log (§9): writing entries with their typed links, posting
//! them to the log channel, and reading them.

use quack_core::ids::{AccountId, GuildId, ModLogId};
use quack_core::modlog::{ModLogEntry, ModLogLinks, ModLogSource};
use quack_db::modlog::{self, NewEntry};
use quack_db::{DbError, Tx};
use quack_telemetry::spans::SpanEvent;

use crate::obs::{event, kv};

/// Who wrote an entry and from where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct By {
    /// The actor; `None` = Quack.
    pub actor: Option<AccountId>,
    /// Where it came from.
    pub source: ModLogSource,
}

impl By {
    /// Quack acting on its own (jobs, gateway events).
    pub(crate) const QUACK: Self = Self {
        actor: None,
        source: ModLogSource::Quack,
    };
}

/// Appends an entry in the caller's transaction (and enqueues its log
/// channel post when it is posted and a log channel is set).
pub(crate) async fn write(
    tx: &mut Tx,
    guild: GuildId,
    entry: ModLogEntry,
    by: By,
    links: ModLogLinks,
) -> Result<ModLogId, DbError> {
    let written = modlog::insert(
        tx,
        &NewEntry {
            guild,
            entry: &entry,
            actor: by.actor,
            source: by.source,
            links,
            legacy_result: None,
        },
    )
    .await?;
    event(
        SpanEvent::ModLogEntry,
        vec![
            kv("entry.id", written.id.get()),
            kv("kind", entry.kind().as_str()),
        ],
    );
    Ok(written.id)
}
