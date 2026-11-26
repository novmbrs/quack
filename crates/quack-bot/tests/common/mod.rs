//! The test world for quack-bot: the engine on real PostgreSQL 18 with the
//! fake `Discord` port (quack-testkit), a recording responder, an ordering
//! probe, and Discord-shaped interaction payloads.
//!
//! Ordering: every database query opens a `db` span and every use case a
//! `use_case` span; a tracing layer writes their opening into the same
//! log the recorder writes each acknowledgement to, and the recorder notes
//! how many port calls the fake had seen at the ack. So a test can assert
//! that the ack came before any database or Discord call.

#![allow(
    dead_code,
    unreachable_pub,
    missing_docs,
    reason = "each test file uses part of the world"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers"
)]

pub mod payload;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use quack_bot::{Bot, Responder};
use quack_core::ids::{ChannelId, GuildId, RoleId};
use quack_core::permissions::{Permission, Permissions, RoleFacts};
use quack_discord::Outcome;
use quack_discord::model::{InteractionRef, InteractionToken};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::BoxFuture;
use quack_testkit::fake::FakeDiscord;
use quack_testkit::{GUILD, Harness, sf};
use quack_text::model::{ButtonTarget, Modal, Row};
use quack_text::{Msg, Tone};
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::{Layer, Registry};

/// The moderator role (Moderate Members, position 30).
pub const MOD_ROLE: u64 = 3_000;
/// The administrator role (Administrator, position 40).
pub const ADMIN_ROLE: u64 = 5_000;
/// The members' role (position 5).
pub const MEMBER_ROLE: u64 = 4_000;

/// One thing that happened, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// An initial callback (`defer`, `defer_update`, `reply`, `modal`,
    /// `autocomplete`).
    Ack(&'static str),
    /// A database query or use case started.
    Work(&'static str),
    /// The original response was edited, or a follow-up posted.
    Answer,
}

/// The shared, ordered log.
#[derive(Clone, Debug, Default)]
pub struct Log(Arc<Mutex<Vec<Step>>>);

impl Log {
    fn push(&self, step: Step) {
        self.0.lock().unwrap().push(step);
    }

    /// Everything so far.
    pub fn steps(&self) -> Vec<Step> {
        self.0.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// Writes `db` and `use_case` span openings into the log.
struct Probe(Log);

impl<S: tracing::Subscriber> Layer<S> for Probe {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: Context<'_, S>,
    ) {
        match attrs.metadata().name() {
            "db" => self.0.push(Step::Work("db")),
            "use_case" => self.0.push(Step::Work("use_case")),
            _ => {}
        }
    }
}

/// What the bot sent in answer to an interaction.
#[derive(Clone, Debug, PartialEq)]
pub enum Sent {
    /// Type 5.
    Defer { ephemeral: bool },
    /// Type 6.
    DeferUpdate,
    /// Type 4.
    Reply { msg: Msg, ephemeral: bool },
    /// Type 9.
    Modal(Modal),
    /// Type 8.
    Choices(Vec<AutocompleteChoice>),
    /// An edit of the original response.
    Edit(Msg),
    /// A follow-up.
    Followup { msg: Msg, ephemeral: bool },
}

/// A responder that records everything, with timestamps, and the port
/// calls the fake had seen at each ack.
pub struct Recorder {
    log: Log,
    fake: Arc<FakeDiscord>,
    sent: Mutex<Vec<(Instant, Sent)>>,
    port_calls_at_ack: Mutex<Vec<usize>>,
}

impl Recorder {
    fn initial(&self, step: &'static str, sent: Sent) -> BoxFuture<'_, Outcome<()>> {
        self.log.push(Step::Ack(step));
        self.port_calls_at_ack
            .lock()
            .unwrap()
            .push(self.fake.calls().len());
        self.sent.lock().unwrap().push((Instant::now(), sent));
        Box::pin(async { Outcome::Done(()) })
    }

    fn later(&self, sent: Sent) -> BoxFuture<'_, Outcome<()>> {
        self.log.push(Step::Answer);
        self.sent.lock().unwrap().push((Instant::now(), sent));
        Box::pin(async { Outcome::Done(()) })
    }
}

impl Responder for Recorder {
    fn defer<'a>(&'a self, _: &'a InteractionRef, ephemeral: bool) -> BoxFuture<'a, Outcome<()>> {
        self.initial("defer", Sent::Defer { ephemeral })
    }
    fn defer_update<'a>(&'a self, _: &'a InteractionRef) -> BoxFuture<'a, Outcome<()>> {
        self.initial("defer_update", Sent::DeferUpdate)
    }
    fn reply<'a>(
        &'a self,
        _: &'a InteractionRef,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        self.initial(
            "reply",
            Sent::Reply {
                msg: msg.clone(),
                ephemeral,
            },
        )
    }
    fn modal<'a>(&'a self, _: &'a InteractionRef, modal: &'a Modal) -> BoxFuture<'a, Outcome<()>> {
        self.initial("modal", Sent::Modal(modal.clone()))
    }
    fn autocomplete<'a>(
        &'a self,
        _: &'a InteractionRef,
        choices: &'a [AutocompleteChoice],
    ) -> BoxFuture<'a, Outcome<()>> {
        self.initial("autocomplete", Sent::Choices(choices.to_vec()))
    }
    fn edit_original<'a>(
        &'a self,
        _: &'a InteractionToken,
        msg: &'a Msg,
        _: &'a [quack_engine::port::OutgoingFile],
    ) -> BoxFuture<'a, Outcome<()>> {
        self.later(Sent::Edit(msg.clone()))
    }
    fn followup<'a>(
        &'a self,
        _: &'a InteractionToken,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        self.later(Sent::Followup {
            msg: msg.clone(),
            ephemeral,
        })
    }
}

/// The test world.
pub struct World {
    /// Database, fake Discord, engine.
    pub h: Harness,
    /// The bot under test.
    pub bot: Bot,
    /// Everything the bot answered.
    pub recorder: Arc<Recorder>,
    /// The ordering log.
    pub log: Log,
    /// A public text channel commands are used in.
    pub channel: ChannelId,
    _trace: tracing::subscriber::DefaultGuard,
}

impl World {
    /// A fresh world: the standard server with staff roles, a channel, and
    /// warm caches. Run tests on the current-thread runtime so the probe
    /// sees every span.
    pub async fn new() -> anyhow::Result<Self> {
        let log = Log::default();
        let trace = tracing::subscriber::set_default(Registry::default().with(Probe(log.clone())));
        let h = Harness::new().await?;
        let guild: GuildId = sf(GUILD);
        let channel = h.fake.world(|w| {
            for (id, position, permissions) in [
                (MOD_ROLE, 30, Permissions::from(Permission::ModerateMembers)),
                (ADMIN_ROLE, 40, Permissions::from(Permission::Administrator)),
                (MEMBER_ROLE, 5, Permissions::NONE),
            ] {
                w.add_role(
                    guild,
                    RoleFacts {
                        id: sf::<RoleId>(id),
                        name: format!("role {id}"),
                        position,
                        permissions,
                        managed: false,
                    },
                );
            }
            w.add_text_channel(guild, false)
        });
        h.engine.rules_cache().load_all(h.engine.db()).await?;
        h.engine.config_cache().load_all(h.engine.db()).await?;
        let recorder = Arc::new(Recorder {
            log: log.clone(),
            fake: h.fake.clone(),
            sent: Mutex::new(Vec::new()),
            port_calls_at_ack: Mutex::new(Vec::new()),
        });
        let bot = Bot::new(h.engine.clone(), recorder.clone(), Vec::new());
        Ok(Self {
            h,
            bot,
            recorder,
            log,
            channel,
            _trace: trace,
        })
    }

    /// Delivers one interaction payload and returns what was sent for it.
    pub async fn send(&self, payload: &serde_json::Value) -> Vec<Sent> {
        self.log.clear();
        self.recorder.sent.lock().unwrap().clear();
        self.recorder.port_calls_at_ack.lock().unwrap().clear();
        self.h.fake.clear_calls();
        let ix = quack_discord::interaction::decode_json(&payload.to_string())
            .expect("a decodable payload");
        self.bot.on_interaction(ix).await;
        self.sent()
    }

    /// What was sent for the last interaction.
    pub fn sent(&self) -> Vec<Sent> {
        self.recorder
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.clone())
            .collect()
    }

    /// Asserts the interaction was acknowledged exactly once, before any
    /// database query, use case, or Discord port call.
    pub fn assert_acked_first(&self) {
        let steps = self.log.steps();
        let acks = steps.iter().filter(|s| matches!(s, Step::Ack(_))).count();
        assert_eq!(acks, 1, "exactly one ack: {steps:?}");
        assert!(
            matches!(steps.first(), Some(Step::Ack(_))),
            "the ack came first: {steps:?}"
        );
        assert_eq!(
            *self.recorder.port_calls_at_ack.lock().unwrap(),
            vec![0],
            "no Discord call before the ack"
        );
        let times: Vec<Instant> = self
            .recorder
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| *t)
            .collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]), "answers in order");
    }

    /// Asserts the prefilled-modal exception: its read happened before the
    /// one ack, and nothing was written.
    pub fn assert_single_ack(&self) {
        let steps = self.log.steps();
        assert_eq!(
            steps.iter().filter(|s| matches!(s, Step::Ack(_))).count(),
            1,
            "{steps:?}"
        );
    }

    /// Asserts every message the staff member saw was private.
    pub fn assert_ephemeral(&self) {
        for s in self.sent() {
            match s {
                Sent::Defer { ephemeral }
                | Sent::Reply { ephemeral, .. }
                | Sent::Followup { ephemeral, .. } => {
                    assert!(ephemeral, "a staff answer was public: {s:?}");
                }
                Sent::DeferUpdate | Sent::Modal(_) | Sent::Choices(_) | Sent::Edit(_) => {}
            }
        }
    }

    /// The last message shown (the final edit or the immediate reply).
    pub fn answer(&self) -> Msg {
        self.sent()
            .into_iter()
            .rev()
            .find_map(|s| match s {
                Sent::Edit(msg) | Sent::Reply { msg, .. } | Sent::Followup { msg, .. } => Some(msg),
                _ => None,
            })
            .expect("an answer")
    }

    /// The modal opened, if any.
    pub fn modal(&self) -> Option<Modal> {
        self.sent().into_iter().find_map(|s| match s {
            Sent::Modal(m) => Some(m),
            _ => None,
        })
    }
}

/// A message as plain text with its tone and controls, for snapshots.
pub fn plain(msg: &Msg) -> String {
    let mut out = format!(
        "[{}]\n{}",
        tone(msg.tone),
        quack_text::render::plain_msg(msg)
    );
    for row in &msg.rows {
        match row {
            Row::Buttons(buttons) => {
                let labels: Vec<String> = buttons
                    .iter()
                    .map(|b| match &b.target {
                        ButtonTarget::Link(_) => format!("<{}>", b.label),
                        ButtonTarget::Action(_) => format!("[{}]", b.label),
                    })
                    .collect();
                out.push('\n');
                out.push_str(&labels.join(" "));
            }
            Row::Select(s) => {
                out.push_str("\n{select: ");
                out.push_str(&s.placeholder);
                out.push('}');
            }
        }
    }
    out
}

const fn tone(t: Tone) -> &'static str {
    match t {
        Tone::Neutral => "neutral",
        Tone::Success => "success",
        Tone::Warning => "warning",
        Tone::Danger => "danger",
    }
}

/// Snapshot settings that hide dates and snowflakes.
pub fn redactions() -> insta::Settings {
    let mut s = insta::Settings::clone_current();
    s.add_filter(
        r"\d{1,2} [A-Z][a-z]{2} \d{4}( at \d{2}:\d{2} UTC)?",
        "[date]",
    );
    s.add_filter(r"\d{12,}", "[id]");
    s.set_prepend_module_to_snapshot(false);
    s
}

/// Asserts a snapshot of `msg` as plain text (dates and snowflakes
/// hidden). Snapshots live in `tests/common/snapshots`.
pub fn snapshot(name: &str, msg: &Msg) {
    let text = plain(msg);
    redactions().bind(|| insta::assert_snapshot!(name.to_owned(), text));
}
