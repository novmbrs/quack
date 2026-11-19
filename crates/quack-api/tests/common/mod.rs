//! The API under test: a fresh database and fake Discord (`Harness`), a
//! fake Discord OAuth server (httpmock), a dashboard directory, sessions
//! for each kind of person, and a small request builder.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "test scenarios read top to bottom and fail loudly"
)]
#![allow(
    dead_code,
    unreachable_pub,
    reason = "a test fixture shared by several test binaries"
)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use httpmock::MockServer;
use quack_api::{ApiConfig, ApiState, DiscordOauthConfig, PublicUrl, Readiness, SessionSecret};
use quack_core::ids::{AccountId, GuildId, RoleId};
use quack_core::permissions::{Permission, Permissions, RoleFacts};
use quack_engine::Origin;
use quack_testkit::fake::{GuildSetup, MemberSetup};
use quack_testkit::{Harness, quack_permissions};
use serde_json::Value;
use tower::ServiceExt as _;

#[allow(unused_imports, reason = "each test binary uses part of the fixture")]
pub use quack_testkit::{
    ADMIN, GUILD, MEMBER, MEMBER_2, MODERATOR, MODERATOR_2, OWNER, rules_for_tests, sf,
};

/// The dashboard's public origin in tests.
pub const PUBLIC: &str = "https://quack.test";
/// A rules manager (by role) in the standard server.
pub const RULES_MANAGER: u64 = 60;
/// The rules manager role.
pub const RULES_ROLE: u64 = 900;
/// A second server.
pub const OTHER_GUILD: u64 = 2002;
/// Its owner, who is not in the standard server.
pub const OUTSIDER: u64 = 70;

/// The people the authorization matrix covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// No session.
    Anonymous,
    /// A member of the server without staff capabilities.
    Member,
    /// Moderate Members.
    Moderator,
    /// A rules manager role only.
    RulesManager,
    /// Administrator.
    Administrator,
    /// The server owner.
    Owner,
    /// A moderator without a 2FA record while the server requires 2FA.
    NoMfa,
    /// Someone from another server.
    Outsider,
}

impl Who {
    pub const ALL: [Self; 8] = [
        Self::Anonymous,
        Self::Member,
        Self::Moderator,
        Self::RulesManager,
        Self::Administrator,
        Self::Owner,
        Self::NoMfa,
        Self::Outsider,
    ];
}

/// A signed-in browser.
#[derive(Clone, Debug)]
pub struct Session {
    /// The session cookie's value.
    pub cookie: String,
    /// The CSRF token.
    pub csrf: String,
    /// The account.
    pub account: u64,
}

/// A response, read whole.
#[derive(Debug)]
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub json: Value,
}

impl Resp {
    /// The error code, when the body is the error envelope.
    pub fn code(&self) -> Option<&str> {
        self.json["error"]["code"].as_str()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// The API under test.
pub struct App {
    pub h: Harness,
    pub discord: MockServer,
    pub state: ApiState,
    pub router: Router,
    pub dashboard: PathBuf,
    pub readiness: Arc<Readiness>,
}

fn dashboard_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dashboard-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><title>Quack</title>",
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/app-3f9a1c.js"),
        "console.log('quack');".repeat(20),
    )
    .unwrap();
    std::fs::write(
        dir.join("favicon.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    std::fs::write(dir.parent().unwrap().join("secret.txt"), "outside").unwrap();
    dir
}

impl App {
    /// A fresh world: the standard server (with a rules manager), a second
    /// server with its own owner, and every readiness check passing.
    pub async fn new() -> Self {
        let h = Harness::new().await.unwrap();
        let guild: GuildId = sf(GUILD);
        let other: GuildId = sf(OTHER_GUILD);
        h.fake.world(|w| {
            w.add_role(
                guild,
                RoleFacts {
                    id: sf(RULES_ROLE),
                    name: "Rules".to_owned(),
                    position: 20,
                    permissions: Permissions::NONE,
                    managed: false,
                },
            );
            w.add_member(
                guild,
                &MemberSetup::new(sf(RULES_MANAGER), 20).roles(vec![sf::<RoleId>(RULES_ROLE)]),
            );
            w.add_guild(&GuildSetup {
                id: other,
                owner: sf(OUTSIDER),
                quack_position: 50,
                quack_permissions: quack_permissions(),
                mfa_required: false,
            });
            w.add_member(other, &MemberSetup::new(sf(OUTSIDER), 60));
        });
        h.install(other).await.unwrap();
        let principal = h.principal(OWNER, Origin::Dashboard);
        let current = quack_engine::settings::get(&h.engine, &principal)
            .await
            .unwrap();
        quack_engine::settings::patch(
            &h.engine,
            &principal,
            &quack_engine::settings::SettingsPatch {
                rules_manager_role_ids: Some(vec![sf(RULES_ROLE)]),
                expected_version: Some(current.version),
                ..Default::default()
            },
            &h.key("setup"),
        )
        .await
        .unwrap();
        Self::finish(h).await
    }

    /// The same app with the modules' services connected, as `serve` does.
    pub fn with_modules(mut self) -> Self {
        let log = quack_activity_log::ActivityLog::new(self.h.engine.clone());
        self.state = self.state.clone().with_modules(quack_api::ModuleServices {
            tickets: Some(quack_tickets::hooks()),
            activity_log: Some(quack_activity_log::hooks()),
            honeypot: Some(quack_honeypot::hooks()),
            activity: Some(log),
        });
        self.router = quack_api::router(self.state.clone());
        self
    }

    async fn finish(h: Harness) -> Self {
        let discord = MockServer::start_async().await;
        let dashboard = dashboard_dir();
        let config = ApiConfig {
            public_url: PublicUrl::parse(PUBLIC).unwrap(),
            session_secret: SessionSecret::new([7; 32]),
            discord: DiscordOauthConfig {
                client_id: "4242".to_owned(),
                client_secret: quack_core::Secret::new("client-secret".to_owned()),
                authorize_url: discord.url("/oauth2/authorize"),
                api_base: discord.url("/api/v10"),
            },
            trusted_proxy_hops: 0,
            dashboard_dir: Some(dashboard.clone()),
        };
        let readiness = Arc::new(Readiness::all_ready());
        let state = ApiState::new(h.engine.clone(), config, readiness.clone()).unwrap();
        let router = quack_api::router(state.clone());
        Self {
            h,
            discord,
            state,
            router,
            dashboard,
            readiness,
        }
    }

    /// Creates a session for `account` directly in the database, with
    /// Quack's 2FA record set to `mfa`.
    pub async fn login(&self, account: u64, mfa: bool) -> Session {
        let raw: [u8; 32] = rand_bytes();
        let cookie = URL_SAFE_NO_PAD.encode(raw);
        let csrf = URL_SAFE_NO_PAD.encode(rand_bytes::<32>());
        let id_hash = sha256(&raw);
        let mut tx = self.h.db.db.begin().await.unwrap();
        let account_id: AccountId = sf(account);
        quack_db::accounts::upsert(&mut tx, account_id, mfa)
            .await
            .unwrap();
        quack_db::sessions::create(
            &mut tx,
            &quack_db::sessions::NewSession {
                id_hash,
                account: account_id,
                csrf_token: csrf.clone(),
                discord_token_ciphertext: vec![0; 40],
                key_fingerprint: SessionSecret::new([7; 32]).fingerprint(),
                expires_at: time::OffsetDateTime::now_utc() + time::Duration::days(1),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Session {
            cookie,
            csrf,
            account,
        }
    }

    /// A session for each kind of person (`None` for anonymous). `NoMfa`
    /// turns on the server's 2FA requirement.
    pub async fn session_for(&self, who: Who) -> Option<Session> {
        match who {
            Who::Anonymous => None,
            Who::Member => Some(self.login(MEMBER, true).await),
            Who::Moderator => Some(self.login(MODERATOR, true).await),
            Who::RulesManager => Some(self.login(RULES_MANAGER, true).await),
            Who::Administrator => Some(self.login(ADMIN, true).await),
            Who::Owner => Some(self.login(OWNER, true).await),
            Who::NoMfa => {
                self.set_mfa_required(true);
                Some(self.login(MODERATOR_2, false).await)
            }
            Who::Outsider => Some(self.login(OUTSIDER, true).await),
        }
    }

    /// Turns the standard server's 2FA requirement on or off.
    pub fn set_mfa_required(&self, on: bool) {
        let guild: GuildId = sf(GUILD);
        self.h.fake.world(|w| {
            if let Some(g) = w.guilds.get_mut(&guild) {
                g.facts.mfa_required = on;
            }
        });
    }

    /// A request to `path`.
    pub fn req(&self, method: Method, path: &str) -> Req {
        Req {
            method,
            path: path.to_owned(),
            session: None,
            body: None,
            key: Some(format!("key-{}", uuid::Uuid::now_v7().simple())),
            origin: Some(PUBLIC.to_owned()),
            csrf: true,
            headers: Vec::new(),
        }
    }

    pub fn get(&self, path: &str) -> Req {
        self.req(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> Req {
        self.req(Method::POST, path)
    }

    /// Sends a request through the whole application.
    pub async fn send(&self, req: Req) -> Resp {
        let mut builder = Request::builder().method(req.method.clone()).uri(&req.path);
        let mut cookies = Vec::new();
        if let Some(s) = &req.session {
            cookies.push(format!("__Host-quack_session={}", s.cookie));
            if req.csrf && req.method != Method::GET {
                builder = builder.header("x-csrf-token", &s.csrf);
            }
        }
        for (name, value) in &req.headers {
            if name.eq_ignore_ascii_case("cookie") {
                cookies.push(value.clone());
            } else {
                builder = builder.header(name.as_str(), value.as_str());
            }
        }
        if !cookies.is_empty() {
            builder = builder.header(header::COOKIE, cookies.join("; "));
        }
        if req.method != Method::GET {
            if let Some(o) = &req.origin {
                builder = builder.header("origin", o);
            }
            if let Some(k) = &req.key {
                builder = builder.header("idempotency-key", k);
            }
        }
        let body = match &req.body {
            Some(json) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(json).unwrap())
            }
            None => Body::empty(),
        };
        let response = self
            .router
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1 << 24)
            .await
            .unwrap()
            .to_vec();
        let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            body,
            json,
        }
    }
}

/// A request being built.
#[derive(Clone, Debug)]
pub struct Req {
    pub method: Method,
    pub path: String,
    pub session: Option<Session>,
    pub body: Option<Value>,
    pub key: Option<String>,
    pub origin: Option<String>,
    pub csrf: bool,
    pub headers: Vec<(String, String)>,
}

impl Req {
    pub fn by(mut self, session: &Session) -> Self {
        self.session = Some(session.clone());
        self
    }

    pub fn by_opt(mut self, session: Option<&Session>) -> Self {
        self.session = session.cloned();
        self
    }

    pub fn json(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }

    pub fn key(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }

    pub fn no_key(mut self) -> Self {
        self.key = None;
        self
    }

    pub fn origin(mut self, origin: Option<&str>) -> Self {
        self.origin = origin.map(str::to_owned);
        self
    }

    pub fn no_csrf(mut self) -> Self {
        self.csrf = false;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0; N];
    aws_lc_rs::rand::fill(&mut out).unwrap();
    out
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    d.as_ref().try_into().unwrap()
}

/// The standard server's path prefix.
pub fn g(rest: &str) -> String {
    format!("/api/guilds/{GUILD}{rest}")
}

/// Permissions of a plain member.
pub fn none() -> Permissions {
    Permissions::NONE
}

pub fn moderate() -> Permissions {
    Permission::ModerateMembers.into()
}

pub fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap()
}
