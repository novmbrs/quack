//! `/case add` forms opened before their draft row exists (§11.6).
//!
//! `/case add` answers with the form's first page at once, before any
//! database call, so the draft row is created by the first page's submit.
//! The modal's custom id names the draft, rule, version, and page but not
//! the member or the receipt choice, so the bot remembers those here when
//! it opens the modal. An entry lives as long as a draft (15 minutes); a
//! submit after a restart finds none and answers `case.draft.expired`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use quack_core::ids::{AccountId, GuildId, MemberId};

/// Most forms remembered.
const CAPACITY: usize = 10_000;
/// How long a form is remembered (a draft's lifetime).
const TTL: Duration = Duration::from_mins(15);

/// What `/case add` knew when it opened a form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FormStart {
    /// The server.
    pub(crate) guild: GuildId,
    /// The moderator.
    pub(crate) actor: AccountId,
    /// The member the case is about.
    pub(crate) member: MemberId,
    /// Whether the receipt is private.
    pub(crate) private: bool,
}

#[derive(Debug, Default)]
struct Inner {
    forms: HashMap<uuid::Uuid, FormStart>,
    order: VecDeque<(Instant, uuid::Uuid)>,
}

/// The forms opened in the last 15 minutes, bounded.
#[derive(Debug, Default)]
pub(crate) struct Forms {
    inner: Mutex<Inner>,
}

impl Forms {
    /// Remembers a form opened now.
    pub(crate) fn opened(&self, draft: uuid::Uuid, start: FormStart) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(&(at, old)) = inner.order.front() {
            if now.saturating_duration_since(at) < TTL && inner.order.len() < CAPACITY {
                break;
            }
            inner.order.pop_front();
            inner.forms.remove(&old);
        }
        inner.forms.insert(draft, start);
        inner.order.push_back((now, draft));
    }

    /// The form's start, when it was opened by this process for `actor`
    /// in `guild`.
    pub(crate) fn get(
        &self,
        draft: uuid::Uuid,
        guild: GuildId,
        actor: AccountId,
    ) -> Option<FormStart> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner
            .forms
            .get(&draft)
            .copied()
            .filter(|f| f.guild == guild && f.actor == actor)
    }
}
