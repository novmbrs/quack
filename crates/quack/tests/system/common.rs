//! The whole system on real PostgreSQL 18 with the fake Discord port: the
//! application `serve` composes (engine, modules, bot, API), fed typed
//! gateway events and interaction payloads directly, with HTTP requests
//! sent through the API's router and the job queue run to quiescence.
//! [`System::restart`] drops every in-process piece and composes them again
//! over the same database and fake Discord, like a process restart.

#![allow(dead_code, reason = "each story uses part of the world")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use quack::compose::App;
use quack::config::{Config, Need, validate};
use quack::serve::Readiness;
use quack_bot::Responder;
use quack_core::ids::{AccountId, ChannelId, GuildId, MessageId, RoleId};
use quack_core::permissions::{AccountFacts, Permission, Permissions, RoleFacts};
use quack_discord::Outcome;
use quack_discord::cache::GuildCache;
use quack_discord::gateway::Event;
use quack_discord::model::{Guild, InteractionRef, InteractionToken, Message};
use quack_discord::rest::AutocompleteChoice;
use quack_engine::BoxFuture;
use quack_testkit::db::TestDb;
use quack_testkit::fake::{Call, FakeDiscord, GuildSetup, MemberSetup};
use quack_testkit::{ADMIN, GUILD, MEMBER, MEMBER_2, MODERATOR, OWNER, QUACK, sf};
use quack_text::Msg;
use quack_text::model::Modal;
use serde_json::{Value, json};
use tower::ServiceExt as _;

pub const MOD_ROLE: u64 = 900;
pub const ADMIN_ROLE: u64 = 901;
pub const MEMBER_ROLE: u64 = 902;
pub const PUBLIC: &str = "https://quack.test";
const SECRET: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

/// What the bot sent for an interaction.
#[derive(Clone, Debug, PartialEq)]
pub enum Sent {
    Defer { ephemeral: bool },
    DeferUpdate,
    Reply { msg: Msg, ephemeral: bool },
    Modal(Modal),
    Choices(Vec<AutocompleteChoice>),
    Edit(Msg),
    Followup { msg: Msg, ephemeral: bool },
}

/// A responder that records every answer.
#[derive(Default)]
pub struct Recorder {
    sent: Mutex<Vec<Sent>>,
}

impl Recorder {
    fn push(&self, sent: Sent) -> BoxFuture<'_, Outcome<()>> {
        self.sent.lock().unwrap().push(sent);
        Box::pin(async { Outcome::Done(()) })
    }
}

impl Responder for Recorder {
    fn defer<'a>(&'a self, _: &'a InteractionRef, ephemeral: bool) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Defer { ephemeral })
    }
    fn defer_update<'a>(&'a self, _: &'a InteractionRef) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::DeferUpdate)
    }
    fn reply<'a>(
        &'a self,
        _: &'a InteractionRef,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Reply {
            msg: msg.clone(),
            ephemeral,
        })
    }
    fn modal<'a>(&'a self, _: &'a InteractionRef, modal: &'a Modal) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Modal(modal.clone()))
    }
    fn autocomplete<'a>(
        &'a self,
        _: &'a InteractionRef,
        choices: &'a [AutocompleteChoice],
    ) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Choices(choices.to_vec()))
    }
    fn edit_original<'a>(
        &'a self,
        _: &'a InteractionToken,
        msg: &'a Msg,
    ) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Edit(msg.clone()))
    }
    fn followup<'a>(
        &'a self,
        _: &'a InteractionToken,
        msg: &'a Msg,
        ephemeral: bool,
    ) -> BoxFuture<'a, Outcome<()>> {
        self.push(Sent::Followup {
            msg: msg.clone(),
            ephemeral,
        })
    }
}

/// Who runs a command or clicks.
#[derive(Clone, Copy, Debug)]
pub struct Who {
    pub id: u64,
    pub role: Option<u64>,
    pub permissions: u64,
}

const fn bits(p: Permission) -> u64 {
    Permissions(p.bit()).0
}

pub const fn moderator() -> Who {
    Who {
        id: MODERATOR,
        role: Some(MOD_ROLE),
        permissions: bits(Permission::ModerateMembers),
    }
}

pub const fn admin() -> Who {
    Who {
        id: ADMIN,
        role: Some(ADMIN_ROLE),
        permissions: bits(Permission::Administrator),
    }
}

pub const fn member() -> Who {
    Who {
        id: MEMBER,
        role: Some(MEMBER_ROLE),
        permissions: 0,
    }
}

/// A signed-in dashboard session.
#[derive(Clone, Debug)]
pub struct Session {
    cookie: String,
    csrf: String,
}

/// An HTTP answer.
#[derive(Debug)]
pub struct Resp {
    pub status: StatusCode,
    pub json: Value,
}

/// The test world.
pub struct System {
    pub db: TestDb,
    pub fake: Arc<FakeDiscord>,
    pub config: Config,
    pub app: App,
    pub recorder: Arc<Recorder>,
    pub ready: Arc<Readiness>,
    pub guild: GuildId,
    /// A public text channel commands are used in.
    pub channel: ChannelId,
    keys: AtomicU64,
}

static SEQ: AtomicU64 = AtomicU64::new(1);
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// A fresh snowflake made now.
pub fn snowflake() -> u64 {
    let now_ms =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    ((now_ms - DISCORD_EPOCH_MS) << 22) | (SEQ.fetch_add(1, Ordering::Relaxed) & 0x3F_FFFF)
}

fn dashboard_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "system-dashboard-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>Quack</title>",
    )
    .unwrap();
    dir
}

fn config(test: &TestDb, extra: &[(&str, String)]) -> Config {
    let name = test.db.connect_options().get_database().unwrap().to_owned();
    let vars = [
        ("database_url", format!("postgres://localhost/{name}")),
        ("discord_token", "token".to_owned()),
        ("discord_application_id", "42".to_owned()),
        ("discord_client_secret", "secret".to_owned()),
        ("public_url", PUBLIC.to_owned()),
        ("session_secret", SECRET.to_owned()),
        (
            "dashboard_dir",
            dashboard_dir().to_str().unwrap().to_owned(),
        ),
        ("listen_addr", "127.0.0.1:0".to_owned()),
    ]
    .into_iter()
    .chain(extra.iter().cloned())
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    validate(Need::Serve, &vars).unwrap()
}

fn roles() -> Vec<RoleFacts> {
    [
        (MOD_ROLE, 30, Permissions::from(Permission::ModerateMembers)),
        (ADMIN_ROLE, 40, Permissions::from(Permission::Administrator)),
        (MEMBER_ROLE, 5, Permissions::NONE),
    ]
    .into_iter()
    .map(|(id, position, permissions)| RoleFacts {
        id: sf::<RoleId>(id),
        name: format!("role {id}"),
        position,
        permissions,
        managed: false,
    })
    .collect()
}

impl System {
    /// The standard server in the fake (not yet installed in Quack) and the
    /// composed application.
    pub async fn boot() -> Result<Self> {
        Self::boot_with(&[]).await
    }

    /// [`System::boot`] with more configuration keys (lowercase, no
    /// prefix).
    pub async fn boot_with(extra: &[(&str, String)]) -> Result<Self> {
        // Before anything records a metric (see `capture`).
        let _ = crate::capture::capture();
        let db = TestDb::with_connections(24).await?;
        let fake = Arc::new(FakeDiscord::new(sf(QUACK)));
        let guild: GuildId = sf(GUILD);
        let channel = fake.world(|w| {
            w.add_guild(&GuildSetup {
                id: guild,
                owner: sf(OWNER),
                quack_position: 50,
                quack_permissions: quack_testkit::quack_permissions(),
                mfa_required: false,
            });
            for role in roles() {
                w.add_role(guild, role);
            }
            w.add_member(guild, &MemberSetup::new(sf(OWNER), 60));
            w.add_member(
                guild,
                &MemberSetup::new(sf(MODERATOR), 30)
                    .permissions(Permission::ModerateMembers)
                    .roles(vec![sf(MOD_ROLE)]),
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(ADMIN), 40)
                    .permissions(Permission::Administrator)
                    .roles(vec![sf(ADMIN_ROLE)]),
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(MEMBER), 5).roles(vec![sf(MEMBER_ROLE)]),
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(MEMBER_2), 5).roles(vec![sf(MEMBER_ROLE)]),
            );
            w.add_text_channel(guild, false)
        });
        let config = config(&db, extra);
        let recorder = Arc::new(Recorder::default());
        let ready = Arc::new(Readiness::default());
        let app = compose(&config, &db, &fake, &recorder, &ready)?;
        Ok(Self {
            db,
            fake,
            config,
            app,
            recorder,
            ready,
            guild,
            channel,
            keys: AtomicU64::new(1),
        })
    }

    /// Drops every in-process piece (caches, the activity log's queue,
    /// the bot) and composes them again over the same database and fake,
    /// then warms the caches the way `serve` does.
    pub async fn restart(&mut self) -> Result<()> {
        self.app = compose(
            &self.config,
            &self.db,
            &self.fake,
            &self.recorder,
            &self.ready,
        )?;
        self.warm().await
    }

    /// Loads the rules and config caches (`serve` step 8).
    pub async fn warm(&self) -> Result<()> {
        let engine = &self.app.engine;
        engine.rules_cache().load_all(engine.db()).await?;
        engine.config_cache().load_all(engine.db()).await?;
        Ok(())
    }

    /// Quack joins the standard server (`GUILD_CREATE`).
    pub async fn install(&self) -> Result<()> {
        let facts = self
            .fake
            .world(|w| w.guilds.get(&self.guild).map(|g| g.facts.clone()))
            .context("the fake server")?;
        let channels: Vec<ChannelId> = self.fake.world(|w| {
            w.guilds
                .get(&self.guild)
                .map_or_else(Vec::new, |g| g.channels.keys().copied().collect())
        });
        self.event(Event::GuildCreate {
            guild: Guild {
                id: self.guild,
                name: facts.name.clone(),
                icon_hash: None,
                owner_id: facts.owner_id,
                mfa_required: false,
                roles: facts.roles.clone(),
            },
            channel_ids: channels,
            thread_ids: Vec::new(),
            member_count: None,
        })
        .await;
        self.warm().await
    }

    /// One gateway event, as the dispatcher hands it to the bot.
    pub async fn event(&self, event: Event) {
        self.app.bot.on_event(event).await;
    }

    /// `author` posts `content` in `channel`: the message exists in the
    /// fake (so history reads find it) and its `MESSAGE_CREATE` reaches
    /// the bot.
    pub async fn say(&self, channel: ChannelId, author: u64, content: &str) -> MessageId {
        let (id, roles) = self.fake.world(|w| {
            let id = w.post(self.guild, channel, sf(author), content, Vec::new());
            let roles = w.member(self.guild, sf(author)).map(|m| m.role_ids.clone());
            (id, roles)
        });
        self.event(Event::MessageCreate(Message {
            id,
            channel_id: channel,
            guild_id: Some(self.guild),
            author: AccountFacts {
                id: sf(author),
                username: format!("user{author}"),
                global_name: None,
                avatar_hash: None,
                bot: false,
            },
            webhook: false,
            member_role_ids: roles,
            content: content.to_owned(),
            created_at: time::OffsetDateTime::now_utc(),
            edited_at: None,
            embeds: Vec::new(),
            attachments: Vec::new(),
        }))
        .await;
        id
    }

    /// `author`'s message `id` in `channel` now reads `content`
    /// (`MESSAGE_UPDATE` with an edited time).
    pub async fn edit(&self, channel: ChannelId, id: MessageId, author: u64, content: &str) {
        let now = time::OffsetDateTime::now_utc();
        self.event(Event::MessageUpdate(Message {
            id,
            channel_id: channel,
            guild_id: Some(self.guild),
            author: AccountFacts {
                id: sf(author),
                username: format!("user{author}"),
                global_name: None,
                avatar_hash: None,
                bot: false,
            },
            webhook: false,
            member_role_ids: None,
            content: content.to_owned(),
            created_at: now,
            edited_at: Some(now),
            embeds: Vec::new(),
            attachments: Vec::new(),
        }))
        .await;
    }

    /// Message `id` in `channel` was deleted (`MESSAGE_DELETE`).
    pub async fn delete(&self, channel: ChannelId, id: MessageId) {
        self.fake.world(|w| w.messages.remove(&id));
        self.event(Event::MessageDelete {
            guild_id: Some(self.guild),
            channel_id: channel,
            message_id: id,
        })
        .await;
    }

    /// Delivers one interaction payload; returns what the bot sent for it.
    pub async fn send(&self, payload: &Value) -> Vec<Sent> {
        self.recorder.sent.lock().unwrap().clear();
        let ix = quack_discord::interaction::decode_json(&payload.to_string())
            .expect("a decodable payload");
        self.app.bot.on_interaction(ix).await;
        self.recorder.sent.lock().unwrap().clone()
    }

    /// The last message the bot answered with (an edit or a reply).
    pub fn answer(&self) -> Msg {
        self.recorder
            .sent
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|s| match s {
                Sent::Edit(m) | Sent::Reply { msg: m, .. } | Sent::Followup { msg: m, .. } => {
                    Some(m.clone())
                }
                _ => None,
            })
            .expect("an answer")
    }

    /// The modal the bot opened, if it did.
    pub fn modal(&self) -> Option<Modal> {
        self.recorder
            .sent
            .lock()
            .unwrap()
            .iter()
            .find_map(|s| match s {
                Sent::Modal(m) => Some(m.clone()),
                _ => None,
            })
    }

    /// Runs the job queue to quiescence with the composed engine and the
    /// modules' handlers, exactly as the runner dispatches them.
    pub async fn settle(&self) -> Result<Vec<quack_testkit::Ran>> {
        quack_testkit::settle_with(&self.app.engine, &self.app.module_jobs()).await
    }

    /// Signs `account` in to the dashboard (a session row, as the OAuth
    /// callback would write it).
    pub async fn login(&self, account: u64) -> Result<Session> {
        let fresh = || {
            let mut out = [0_u8; 32];
            out[..16].copy_from_slice(uuid::Uuid::now_v7().as_bytes());
            out[16..].copy_from_slice(uuid::Uuid::now_v7().as_bytes());
            out
        };
        let raw = fresh();
        let cookie = URL_SAFE_NO_PAD.encode(raw);
        let csrf = URL_SAFE_NO_PAD.encode(fresh());
        let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &raw);
        let mut id_hash = [0_u8; 32];
        id_hash.copy_from_slice(digest.as_ref());
        let mut tx = self.db.db.begin().await?;
        let account_id: AccountId = sf(account);
        quack_db::accounts::upsert(&mut tx, account_id, true).await?;
        quack_db::sessions::create(
            &mut tx,
            &quack_db::sessions::NewSession {
                id_hash,
                account: account_id,
                csrf_token: csrf.clone(),
                discord_token_ciphertext: vec![0; 40],
                key_fingerprint: self.config.session_key_fingerprint(),
                expires_at: time::OffsetDateTime::now_utc() + time::Duration::days(1),
            },
        )
        .await?;
        tx.commit().await?;
        Ok(Session { cookie, csrf })
    }

    /// One HTTP request through the whole application.
    pub async fn http(
        &self,
        method: Method,
        path: &str,
        session: Option<&Session>,
        body: Option<Value>,
    ) -> Resp {
        let mut builder = Request::builder().method(method.clone()).uri(path);
        if let Some(s) = session {
            builder = builder.header(header::COOKIE, format!("__Host-quack_session={}", s.cookie));
            if method != Method::GET {
                builder = builder.header("x-csrf-token", &s.csrf);
            }
        }
        if method != Method::GET {
            builder = builder.header("origin", PUBLIC).header(
                "idempotency-key",
                format!(
                    "system-key-{:08}",
                    self.keys.fetch_add(1, Ordering::Relaxed)
                ),
            );
        }
        let body = match body {
            Some(json) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&json).unwrap())
            }
            None => Body::empty(),
        };
        let response = self
            .app
            .router
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap();
        Resp {
            status,
            json: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// The plain text of every message Quack posted in `channel` (latest
    /// version of each).
    pub fn posted(&self, channel: ChannelId) -> Vec<String> {
        self.fake.world(|w| {
            w.sent_to(channel)
                .iter()
                .filter_map(|m| m.msg.as_ref().map(quack_text::render::plain_msg))
                .collect()
        })
    }

    /// The plain text of every DM Quack sent `account`.
    pub fn dms(&self, account: u64) -> Vec<String> {
        self.fake.world(|w| {
            w.dms_to(sf(account))
                .iter()
                .filter_map(|m| m.msg.as_ref().map(quack_text::render::plain_msg))
                .collect()
        })
    }

    /// The story never asked Discord for the same effect twice: no member
    /// timed out, kicked, banned, or unbanned twice for one reason, and no
    /// message created twice with one nonce.
    pub fn assert_no_duplicate_effects(&self) {
        let calls = self.fake.calls();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for c in &calls {
            if c.scripted.is_some() {
                continue;
            }
            let key = match c.call {
                Call::Timeout | Call::Kick | Call::Ban | Call::Unban | Call::RemoveTimeout => {
                    format!("{:?} {:?} {:?}", c.call, c.member, c.reason)
                }
                Call::Send => match &c.nonce {
                    Some(n) => format!("send {n:?}"),
                    None => continue,
                },
                Call::DeleteMessage => format!("delete {:?}", c.message),
                _ => continue,
            };
            *seen.entry(key).or_default() += 1;
        }
        let repeated: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
        assert!(
            repeated.is_empty(),
            "repeated Discord effects: {repeated:#?}"
        );
    }
}

fn compose(
    config: &Config,
    db: &TestDb,
    fake: &Arc<FakeDiscord>,
    recorder: &Arc<Recorder>,
    ready: &Arc<Readiness>,
) -> Result<App> {
    Ok(App::compose(
        config,
        db.db.clone(),
        fake.clone(),
        recorder.clone(),
        Arc::new(GuildCache::new()),
        ready,
        None,
    )?)
}

/// A user object.
pub fn user(id: u64) -> Value {
    json!({
        "id": id.to_string(), "username": format!("user{id}"), "discriminator": "0",
        "global_name": null, "avatar": null, "bot": false
    })
}

/// An interaction payload of `kind` (2 command, 3 component, 5 modal
/// submit) in the standard server's `channel`.
pub fn ix(kind: u8, who: Who, channel: ChannelId, data: &Value) -> Value {
    let id = snowflake();
    json!({
        "id": id.to_string(), "application_id": "42", "type": kind,
        "token": format!("token-{id}"), "version": 1,
        "guild_id": GUILD.to_string(),
        "channel": {"id": channel.to_string(), "type": 0},
        "channel_id": channel.to_string(),
        "member": {
            "user": user(who.id),
            "roles": who.role.map_or_else(Vec::new, |r| vec![r.to_string()]),
            "joined_at": "2024-01-02T03:04:05.000000+00:00",
            "deaf": false, "mute": false, "flags": 0, "pending": false,
            "permissions": who.permissions.to_string(), "nick": null,
            "communication_disabled_until": null
        },
        "app_permissions": "8", "locale": "en-US", "guild_locale": "en-US",
        "entitlements": [], "authorizing_integration_owners": {"0": GUILD.to_string()},
        "context": 0, "attachment_size_limit": 10_485_760,
        "data": data
    })
}

/// A top-level slash command with options.
pub fn slash(name: &str, options: &Value) -> Value {
    json!({"id": "101", "name": name, "type": 1, "options": options})
}

/// `/case add member rule [private]`.
pub fn case_add(member: u64, rule: &str, private: Option<bool>) -> Value {
    let mut options = vec![
        json!({"name": "member", "type": 6, "value": member.to_string()}),
        json!({"name": "rule", "type": 3, "value": rule}),
    ];
    if let Some(p) = private {
        options.push(json!({"name": "private", "type": 5, "value": p}));
    }
    json!({
        "id": "100", "name": "case", "type": 1,
        "options": [{"name": "add", "type": 1, "options": options}],
        "resolved": {
            "users": {member.to_string(): user(member)},
            "members": {member.to_string(): {
                "roles": [MEMBER_ROLE.to_string()],
                "joined_at": "2024-01-02T03:04:05.000000+00:00",
                "permissions": "0", "nick": null, "flags": 0, "pending": false,
                "communication_disabled_until": null
            }}
        }
    })
}

/// A select (3 string, 8 channel) with this custom id and values, by
/// `who`, on a message in `channel`.
pub fn select(who: Who, channel: ChannelId, custom_id: &str, kind: u8, values: &[String]) -> Value {
    let data = json!({"custom_id": custom_id, "component_type": kind, "values": values});
    on_message(ix(3, who, channel, &data), channel, true)
}

/// Attaches the message a component is on.
pub fn on_message(mut payload: Value, channel: ChannelId, ephemeral: bool) -> Value {
    payload["message"] = json!({
        "id": snowflake().to_string(), "channel_id": channel.to_string(),
        "author": user(QUACK), "content": "", "timestamp": "2026-11-17T12:00:00.000000+00:00",
        "edited_timestamp": null, "tts": false, "mention_everyone": false, "mentions": [],
        "mention_roles": [], "attachments": [], "embeds": [], "pinned": false, "type": 0,
        "flags": if ephemeral { 64 } else { 0 }
    });
    payload
}

/// A click on a button with `custom_id` by `who`, on a message in `channel`.
pub fn click(who: Who, channel: ChannelId, custom_id: &str, ephemeral: bool) -> Value {
    let data = json!({"custom_id": custom_id, "component_type": 2, "values": []});
    on_message(ix(3, who, channel, &data), channel, ephemeral)
}

/// A modal submit with text inputs `(custom id, value)`.
pub fn modal_submit(
    who: Who,
    channel: ChannelId,
    custom_id: &str,
    inputs: &[(&str, &str)],
) -> Value {
    let components: Vec<Value> = inputs
        .iter()
        .enumerate()
        .map(|(i, (id, value))| {
            json!({"type": 18, "id": i * 2 + 1, "component":
                {"type": 4, "id": i * 2 + 2, "custom_id": id, "value": value}})
        })
        .collect();
    let data = json!({"custom_id": custom_id, "components": components});
    on_message(ix(5, who, channel, &data), channel, true)
}
