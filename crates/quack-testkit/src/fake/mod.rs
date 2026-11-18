//! A fake `Discord` port: in-memory servers, members, roles, channels,
//! bans, and messages that behave like Discord, every call recorded, and
//! any call's outcome scriptable.
//!
//! By default every call does what Discord would. A test queues
//! [`Script`]s per [`Call`] kind; each call takes the next one:
//!
//! - `Refuse`, `RetryAfter`, `NotSent`: nothing changes.
//! - `Unknown`: nothing changes, the caller can't tell.
//! - `LoseResponse`: the call takes effect, but the caller sees `Unknown`.
//! - `Hold` / `HoldAfter`: the call stops before (or after) taking effect
//!   until the test releases its [`Gate`]. Aborting the caller while it is
//!   held is a crash at exactly that point.
//! - `Late` (moderation calls): the request reached Discord, the caller
//!   sees `Unknown`, and the change lands only when the test calls
//!   [`FakeDiscord::land_late`], after anything else that happens meanwhile.

mod effects;
mod port;
mod run;
mod state;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use quack_core::ids::{AccountId, ChannelId, GuildId, MemberId, MessageId};
use quack_engine::port::{Nonce, Refusal, RefusalKind};
use quack_text::Msg;
use time::OffsetDateTime;
use tokio::sync::Notify;

pub use state::{
    FakeMessage, GuildSetup, GuildState, MemberSetup, PRIVATE_THREAD, World, everyone_permissions,
};

/// Every kind of port call, for scripting and assertions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Call {
    /// `guild_facts`.
    GuildFacts,
    /// `member` (cached or REST).
    Member,
    /// `accounts`.
    Accounts,
    /// `channel_facts`.
    ChannelFacts,
    /// `guild_channels`.
    GuildChannels,
    /// `ban` (the read).
    GetBan,
    /// `is_thread_member`.
    IsThreadMember,
    /// `search_members`.
    SearchMembers,
    /// `timeout`.
    Timeout,
    /// `remove_timeout`.
    RemoveTimeout,
    /// `kick`.
    Kick,
    /// `ban_member`.
    Ban,
    /// `unban`.
    Unban,
    /// `open_dm`.
    OpenDm,
    /// `send`.
    Send,
    /// `edit`.
    Edit,
    /// `edit_interaction_original`.
    EditOriginal,
    /// `delete_message`.
    DeleteMessage,
    /// `fetch_message`.
    FetchMessage,
    /// `fetch_history`.
    FetchHistory,
    /// `download_attachment`.
    Download,
    /// `create_channel`.
    CreateChannel,
    /// `create_private_thread`.
    CreateThread,
    /// `find_threads`.
    FindThreads,
    /// `add_thread_member`.
    AddThreadMember,
    /// `remove_thread_member`.
    RemoveThreadMember,
    /// `thread_members`.
    ThreadMembers,
    /// `set_thread_state`.
    SetThreadState,
    /// `delete_channel`.
    DeleteChannel,
}

/// How one scripted call goes.
#[derive(Clone, Debug)]
pub enum Script {
    /// The call behaves normally (to script a later call of the kind).
    Pass,
    /// Discord definitely refuses; nothing changes.
    Refuse(Refusal),
    /// Rate limited before sending; nothing changes.
    RetryAfter(Duration),
    /// Never sent; nothing changes.
    NotSent,
    /// The result is unknown and nothing changed.
    Unknown,
    /// The call takes effect, but its answer is lost (`Unknown`).
    LoseResponse,
    /// The call waits before taking effect until the gate is released,
    /// then behaves normally.
    Hold(Gate),
    /// The call takes effect, then waits until the gate is released, then
    /// answers normally.
    HoldAfter(Gate),
    /// Moderation calls only: the caller sees `Unknown` and the change
    /// lands later, when the test calls [`FakeDiscord::land_late`].
    Late,
    /// [`Script::HoldAfter`] for the first call of its kind in this
    /// channel; calls in other channels pass it by.
    HoldAfterIn(Gate, ChannelId),
}

impl Script {
    /// A refusal of this kind with its usual status and code.
    pub const fn refuse(kind: RefusalKind) -> Self {
        Self::Refuse(Refusal::of(kind))
    }
}

/// A point a held call waits at.
#[derive(Clone, Debug, Default)]
pub struct Gate {
    inner: Arc<GateInner>,
}

#[derive(Debug, Default)]
struct GateInner {
    reached: Mutex<bool>,
    reached_notify: Notify,
    released: Mutex<bool>,
    released_notify: Notify,
}

impl Gate {
    /// A new, closed gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Waits until a call reaches the gate.
    pub async fn reached(&self) {
        loop {
            let notified = self.inner.reached_notify.notified();
            if *lock(&self.inner.reached) {
                return;
            }
            notified.await;
        }
    }

    /// Lets the held call continue.
    pub fn release(&self) {
        *lock(&self.inner.released) = true;
        self.inner.released_notify.notify_waiters();
    }

    pub(crate) async fn hold(&self) {
        *lock(&self.inner.reached) = true;
        self.inner.reached_notify.notify_waiters();
        loop {
            let notified = self.inner.released_notify.notified();
            if *lock(&self.inner.released) {
                return;
            }
            notified.await;
        }
    }
}

/// One recorded call.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// What was called.
    pub call: Call,
    /// The server, when the call names one.
    pub guild: Option<GuildId>,
    /// The member or account, when the call names one.
    pub member: Option<MemberId>,
    /// The channel, when the call names one.
    pub channel: Option<ChannelId>,
    /// The message, when the call names one.
    pub message: Option<MessageId>,
    /// The audit log reason.
    pub reason: Option<String>,
    /// A timeout's end.
    pub until: Option<OffsetDateTime>,
    /// A create's nonce.
    pub nonce: Option<Nonce>,
    /// The message sent or edited.
    pub msg: Option<Msg>,
    /// How the script answered it (`None`: normally).
    pub scripted: Option<String>,
}

impl Recorded {
    pub(crate) fn new(call: Call) -> Self {
        Self {
            call,
            guild: None,
            member: None,
            channel: None,
            message: None,
            reason: None,
            until: None,
            nonce: None,
            msg: None,
            scripted: None,
        }
    }
}

/// A change Discord accepted but hasn't applied yet ([`Script::Late`]).
pub(crate) struct LateEffect(pub(crate) Box<dyn FnOnce(&mut World) + Send>);

impl std::fmt::Debug for LateEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LateEffect")
    }
}

/// The fake. Share it with `Arc` between the engine and the test.
#[derive(Debug)]
pub struct FakeDiscord {
    world: Mutex<World>,
    scripts: Mutex<HashMap<Call, VecDeque<Script>>>,
    calls: Mutex<Vec<Recorded>>,
    late: Mutex<Vec<LateEffect>>,
}

impl FakeDiscord {
    /// A fake with no servers. Quack's own account is `quack`.
    pub fn new(quack: AccountId) -> Self {
        Self {
            world: Mutex::new(World::new(quack)),
            scripts: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            late: Mutex::new(Vec::new()),
        }
    }

    /// Changes the world (servers, members, bans, messages) directly, as
    /// if it happened on Discord outside Quack.
    pub fn world<T>(&self, change: impl FnOnce(&mut World) -> T) -> T {
        change(&mut lock(&self.world))
    }

    /// Queues `script` for the next call of `call`.
    pub fn script(&self, call: Call, script: Script) {
        lock(&self.scripts)
            .entry(call)
            .or_default()
            .push_back(script);
    }

    /// Queues the same script for the next `n` calls of `call`.
    pub fn script_n(&self, call: Call, n: usize, script: &Script) {
        for _ in 0..n {
            self.script(call, script.clone());
        }
    }

    /// Holds the next call of `call` before it takes effect.
    pub fn hold(&self, call: Call) -> Gate {
        let gate = Gate::new();
        self.script(call, Script::Hold(gate.clone()));
        gate
    }

    /// Holds the next call of `call` after it takes effect.
    pub fn hold_after(&self, call: Call) -> Gate {
        let gate = Gate::new();
        self.script(call, Script::HoldAfter(gate.clone()));
        gate
    }

    /// Applies every change that was answered `Unknown` under
    /// [`Script::Late`], in the order they were sent. Returns how many
    /// landed.
    pub fn land_late(&self) -> usize {
        let late = std::mem::take(&mut *lock(&self.late));
        let n = late.len();
        for effect in late {
            self.world(|w| (effect.0)(w));
        }
        n
    }

    /// How many [`Script::Late`] changes are still waiting to land.
    pub fn late_count(&self) -> usize {
        lock(&self.late).len()
    }

    /// Discord's nonce memory ends: a create repeating an earlier nonce now
    /// makes a new message (what happens once the window has passed).
    pub fn expire_nonces(&self) {
        self.world(|w| {
            for m in w.messages.values_mut() {
                m.nonce = None;
            }
        });
    }

    /// Holds the next call of `call` in `channel` after it takes effect;
    /// calls in other channels go on normally.
    pub fn hold_after_in(&self, call: Call, channel: ChannelId) -> Gate {
        let gate = Gate::new();
        self.script(call, Script::HoldAfterIn(gate.clone(), channel));
        gate
    }

    /// Drops every queued script.
    pub fn clear_scripts(&self) {
        lock(&self.scripts).clear();
    }

    /// Every recorded call, in order.
    pub fn calls(&self) -> Vec<Recorded> {
        lock(&self.calls).clone()
    }

    /// The recorded calls of one kind.
    pub fn calls_of(&self, call: Call) -> Vec<Recorded> {
        lock(&self.calls)
            .iter()
            .filter(|c| c.call == call)
            .cloned()
            .collect()
    }

    /// How many calls of one kind were made.
    pub fn count(&self, call: Call) -> usize {
        lock(&self.calls).iter().filter(|c| c.call == call).count()
    }

    /// Forgets the recorded calls.
    pub fn clear_calls(&self) {
        lock(&self.calls).clear();
    }

    fn next_script(&self, recorded: &Recorded) -> Option<Script> {
        let mut scripts = lock(&self.scripts);
        let queue = scripts.get_mut(&recorded.call)?;
        if let Some(Script::HoldAfterIn(_, channel)) = queue.front()
            && recorded.channel != Some(*channel)
        {
            return None;
        }
        queue.pop_front()
    }

    fn record(&self, recorded: Recorded) {
        lock(&self.calls).push(recorded);
    }
}

/// Locks a mutex, recovering from a poisoned one (a panicking test thread).
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
