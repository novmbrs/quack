//! Building a ticket's transcript (§13.2.4 step 2): the journal merged
//! with the thread's history by message ID (journal text wins, history adds
//! what the journal lacks), rendered with `ticket.transcript.*` and cut at
//! 8 MiB. The journal is read page by page and the history is at most
//! 1,000 messages, so a long ticket is never loaded whole.

use quack_core::ids::{AccountId, MessageId};
use quack_core::limits::MAX_TRANSCRIPT_BYTES;
use quack_core::modules::TranscriptPartial;
use quack_db::tickets::{self, JOURNAL_PAGE, JournalFile, JournalMessage, Ticket};
use quack_db::{DbError, PgConnection};
use quack_engine::port::FetchedMessage;
use quack_text::ticket as text;
use time::OffsetDateTime;

/// History pages the close job reads at most (newest first).
pub(crate) const HISTORY_PAGES: usize = 10;
/// Messages per history page.
pub(crate) const HISTORY_PAGE: u8 = 100;
/// Bytes kept free for the closing partial note.
const NOTE_RESERVE: usize = 256;

/// The thread's history, oldest first.
#[derive(Debug, Default)]
pub(crate) struct History {
    /// The messages.
    pub messages: Vec<FetchedMessage>,
    /// The history reached the thread's first message.
    pub reached_first: bool,
}

/// A rendered transcript.
#[derive(Debug)]
pub(crate) struct Built {
    /// The text (at most 8 MiB).
    pub content: String,
    /// Why it may be missing messages.
    pub partial: Option<TranscriptPartial>,
}

/// The header's facts.
pub(crate) struct Header<'a> {
    /// The server's name.
    pub server: &'a str,
    /// The member's name, when known.
    pub member_name: Option<String>,
    /// When it closed.
    pub closed: OffsetDateTime,
}

/// One transcript entry from either source.
struct Entry {
    id: MessageId,
    at: OffsetDateTime,
    author: AccountId,
    author_name: String,
    text: String,
    files: Vec<JournalFile>,
}

impl From<JournalMessage> for Entry {
    fn from(m: JournalMessage) -> Self {
        Self {
            id: m.message,
            at: m.sent_at,
            author: m.author,
            author_name: m.author_name,
            text: m.text,
            files: m.files,
        }
    }
}

impl From<&FetchedMessage> for Entry {
    fn from(m: &FetchedMessage) -> Self {
        Self {
            id: m.id,
            at: m.created_at,
            author: m.author_id,
            author_name: m.author_name.clone(),
            text: m.content.clone(),
            files: m
                .attachments
                .iter()
                .map(|a| JournalFile {
                    name: a.filename.clone(),
                    size: a.size_bytes,
                })
                .collect(),
        }
    }
}

/// Builds the transcript of `t`. `history` is `None` when Discord didn't
/// let Quack read the thread.
pub(crate) async fn build(
    conn: &mut PgConnection,
    t: &Ticket,
    header: Header<'_>,
    history: Option<&History>,
) -> Result<Built, DbError> {
    let history_messages = history.map_or(&[][..], |h| h.messages.as_slice());
    let complete = history.is_some_and(|h| h.reached_first) || !t.journal_gaps;
    let mut page = tickets::journal(conn, t.guild, t.id, None, JOURNAL_PAGE).await?;
    let member_name = header
        .member_name
        .or_else(|| name_of(t.member.account(), &page, history_messages))
        .unwrap_or_else(|| t.member.get().to_string());
    let mut out = Out::new();
    out.push(&format!(
        "{}\n\n",
        text::transcript_header(
            header.server,
            t.opened_at.unwrap_or(t.created_at),
            &member_name,
            t.member.get(),
            header.closed,
        )
    ));
    let mut journal_done = page.len() < JOURNAL_PAGE as usize;
    let mut from_history = history_messages.iter().peekable();
    let mut from_journal = std::mem::take(&mut page).into_iter().peekable();
    // The last entry written; entries come in ascending ID order, so when
    // the journal page runs out this is its last row and the next page
    // starts after it.
    let mut last: Option<MessageId> = None;
    while !out.full {
        if from_journal.peek().is_none() && !journal_done {
            let next = tickets::journal(conn, t.guild, t.id, last, JOURNAL_PAGE).await?;
            journal_done = next.len() < JOURNAL_PAGE as usize;
            from_journal = next.into_iter().peekable();
        }
        let entry = match (from_journal.peek(), from_history.peek()) {
            (None, None) => break,
            (Some(kept), Some(seen)) if seen.id < kept.message => {
                from_history.next().map(Entry::from)
            }
            (Some(kept), Some(seen)) if seen.id == kept.message => {
                // Journal text wins: it is the original wording.
                from_history.next();
                from_journal.next().map(Entry::from)
            }
            (Some(_), _) => from_journal.next().map(Entry::from),
            (None, Some(_)) => from_history.next().map(Entry::from),
        };
        let Some(entry) = entry else { break };
        if last.is_some_and(|prev| entry.id <= prev) {
            continue;
        }
        last = Some(entry.id);
        out.push(&render(&entry));
    }
    let partial = if out.full {
        Some(TranscriptPartial::Trimmed)
    } else if complete {
        None
    } else {
        Some(TranscriptPartial::History)
    };
    let mut content = out.text;
    if let Some(p) = partial {
        content.push('\n');
        content.push_str(&text::transcript_partial(p));
        content.push('\n');
    }
    Ok(Built { content, partial })
}

/// The member's name from the messages at hand.
fn name_of(
    member: AccountId,
    journal: &[JournalMessage],
    history: &[FetchedMessage],
) -> Option<String> {
    journal
        .iter()
        .find(|m| m.author == member)
        .map(|m| m.author_name.clone())
        .or_else(|| {
            history
                .iter()
                .find(|m| m.author_id == member)
                .map(|m| m.author_name.clone())
        })
}

fn render(e: &Entry) -> String {
    let mut s = text::transcript_line(e.at, &e.author_name, e.author.get(), &e.text);
    s.push('\n');
    for f in &e.files {
        s.push_str(&text::transcript_file_line(&f.name, f.size));
        s.push('\n');
    }
    s
}

/// The transcript text under its size cap.
struct Out {
    text: String,
    full: bool,
}

impl Out {
    fn new() -> Self {
        Self {
            text: String::new(),
            full: false,
        }
    }

    /// Appends `chunk`, cut at a UTF-8 boundary when it would pass the cap.
    fn push(&mut self, chunk: &str) {
        let cap = usize::try_from(MAX_TRANSCRIPT_BYTES).unwrap_or(usize::MAX) - NOTE_RESERVE;
        let room = cap.saturating_sub(self.text.len());
        if chunk.len() <= room {
            self.text.push_str(chunk);
            return;
        }
        let mut end = room;
        while !chunk.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&chunk[..end]);
        self.full = true;
    }
}
