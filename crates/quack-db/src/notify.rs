//! `NOTIFY` channels and the listener that receives them.
//!
//! Channels: `quack_jobs` (payload: job kind), `quack_rules` and
//! `quack_settings` (payload: guild ID). Notifications are only wake-ups;
//! the rows are the truth, so a missed one costs at most a poll interval.
//! PostgreSQL delivers them on commit and folds duplicates within one
//! transaction.

use quack_core::ids::GuildId;
use sqlx::PgConnection;
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};

use crate::DbError;
use crate::jobs::JobKind;
use crate::obs::{self, Notify};

/// The job queue's channel.
pub const JOBS_CHANNEL: &str = "quack_jobs";
/// The rules cache's channel.
pub const RULES_CHANNEL: &str = "quack_rules";
/// The config cache's channel.
pub const SETTINGS_CHANNEL: &str = "quack_settings";

/// Tells every process that `guild`'s rules changed (on commit).
pub async fn rules_changed(conn: &mut PgConnection, guild: GuildId) -> Result<(), DbError> {
    let payload = guild.to_string();
    obs::run(
        Notify,
        "quack_rules",
        sqlx::query!("SELECT pg_notify('quack_rules', $1)", payload).execute(conn),
    )
    .await?;
    Ok(())
}

/// Tells every process that `guild`'s settings or module settings changed
/// (on commit).
pub async fn settings_changed(conn: &mut PgConnection, guild: GuildId) -> Result<(), DbError> {
    let payload = guild.to_string();
    obs::run(
        Notify,
        "quack_settings",
        sqlx::query!("SELECT pg_notify('quack_settings', $1)", payload).execute(conn),
    )
    .await?;
    Ok(())
}

/// One wake-up from the listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notification {
    /// A job of this kind was enqueued.
    Job(JobKind),
    /// A server's rules changed.
    Rules(GuildId),
    /// A server's settings changed.
    Settings(GuildId),
    /// The listener reconnected; notifications may have been missed, so
    /// every consumer should reload or run a pass now.
    Reconnected,
}

/// The one dedicated `LISTEN` connection, outside the pool.
#[derive(Debug)]
pub struct Listener {
    inner: PgListener,
}

impl Listener {
    /// Connects and listens on all three channels. Listening starts before
    /// the first scan, so nothing committed after this call is missed.
    pub async fn connect(options: &PgConnectOptions) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect_lazy_with(options.clone().application_name("quack-listener"));
        let mut inner = PgListener::connect_with(&pool).await?;
        inner.ignore_pool_close_event(true);
        inner
            .listen_all([JOBS_CHANNEL, RULES_CHANNEL, SETTINGS_CHANNEL])
            .await?;
        Ok(Self { inner })
    }

    /// Waits for the next notification. A lost connection is re-established
    /// (and listened again) before [`Notification::Reconnected`] is
    /// returned; an `Err` means reconnecting failed, and the next call tries
    /// again. Payloads that don't parse are skipped.
    pub async fn recv(&mut self) -> Result<Notification, DbError> {
        loop {
            let Some(notification) = self.inner.try_recv().await? else {
                return Ok(Notification::Reconnected);
            };
            if let Some(parsed) = parse(notification.channel(), notification.payload()) {
                return Ok(parsed);
            }
        }
    }
}

fn parse(channel: &str, payload: &str) -> Option<Notification> {
    match channel {
        JOBS_CHANNEL => payload.parse().ok().map(Notification::Job),
        RULES_CHANNEL => payload.parse().ok().map(Notification::Rules),
        SETTINGS_CHANNEL => payload.parse().ok().map(Notification::Settings),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_payloads() {
        assert_eq!(
            parse(JOBS_CHANNEL, "send_dm"),
            Some(Notification::Job(JobKind::SendDm))
        );
        assert_eq!(
            parse(RULES_CHANNEL, "42"),
            GuildId::new(42).map(Notification::Rules)
        );
        assert_eq!(parse(SETTINGS_CHANNEL, "0"), None);
        assert_eq!(parse(JOBS_CHANNEL, "nope"), None);
        assert_eq!(parse("other", "1"), None);
    }
}
