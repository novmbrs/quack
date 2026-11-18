//! One server's import: one PostgreSQL transaction with its
//! `import_guilds` row (§19.2), in the order foreign keys need, then the
//! relationship checks, then commit (or rollback in a dry run).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use quack_core::ids::GuildId;
use quack_db::{Db, Tx};
use time::OffsetDateTime;
use tracing::Instrument as _;
use uuid::Uuid;

use crate::report::Fragment;
use crate::source::Source;
use crate::{ImportError, checks, tables};

/// A server as the `guilds` table names it.
#[derive(Clone, Debug)]
pub(crate) struct GuildKey {
    /// The internal Go ULID every child row uses.
    pub ulid: String,
    /// The Discord server ID.
    pub id: i64,
}

/// What later tables need to know about an imported case.
#[derive(Clone, Debug)]
pub(crate) struct CaseInfo {
    pub member: i64,
    pub voided: bool,
    pub created_at: OffsetDateTime,
    pub timeout_seconds: Option<i32>,
    pub ban_delete_seconds: Option<i32>,
}

/// An imported action.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActionInfo {
    pub case: Uuid,
}

/// An imported appeal.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AppealInfo {
    pub case: Uuid,
    pub member: i64,
    pub open: bool,
    /// Go's `review_message_discord_id`: a candidate for the appeal post.
    pub review_message: Option<i64>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// An imported ticket.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TicketInfo {
    pub thread: Option<i64>,
}

/// Everything one server's import shares.
pub(crate) struct Ctx<'a> {
    pub src: &'a mut Source,
    pub tx: Tx,
    pub g: GuildKey,
    pub f: Fragment,
    pub evaluated_at: OffsetDateTime,
    /// Imported rules (by mapped id).
    pub rules: HashSet<Uuid>,
    /// A template's mapped ULID → the rule it became (§19.5).
    pub rule_ref: HashMap<Uuid, Uuid>,
    /// Every written `(rule, version)` → its levels' `from_case_count`.
    pub versions: HashMap<(Uuid, u32), Vec<u32>>,
    pub cases: HashMap<Uuid, CaseInfo>,
    /// Source case number of every imported case, for the counter.
    pub max_case_number: u64,
    /// `idempotency_key → case` for Go honeypot keys (§19.9).
    pub honeypot_keys: HashMap<String, Uuid>,
    pub actions: HashMap<Uuid, ActionInfo>,
    pub appeals: HashMap<Uuid, AppealInfo>,
    /// Appeal event → its appeal.
    pub appeal_events: HashMap<Uuid, Uuid>,
    pub tickets: HashMap<Uuid, TicketInfo>,
    /// Settings later tables read.
    pub appeal_channel: Option<i64>,
    pub ticket_entry_channel: Option<i64>,
}

impl Ctx<'_> {
    /// The `table:key` example key for the report.
    pub(crate) fn key(&self, key: &str) -> String {
        format!("{}:{key}", self.g.id)
    }
}

/// How the server's transaction ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Finish {
    /// Commit, with the `import_guilds` row.
    Commit,
    /// Roll back after every write and check (dry run).
    Rollback,
}

/// Imports one server. On error the transaction is rolled back and the
/// target is untouched for this server.
pub(crate) async fn import(
    db: &Db,
    src: &mut Source,
    g: GuildKey,
    run: Uuid,
    fingerprint: &str,
    evaluated_at: OffsetDateTime,
    finish: Finish,
) -> Result<Fragment, ImportError> {
    let span = tracing::info_span!("import guild", quack.guild_id = g.id);
    import_in(db, src, g, run, fingerprint, evaluated_at, finish)
        .instrument(span)
        .await
}

async fn import_in(
    db: &Db,
    src: &mut Source,
    g: GuildKey,
    run: Uuid,
    fingerprint: &str,
    evaluated_at: OffsetDateTime,
    finish: Finish,
) -> Result<Fragment, ImportError> {
    let mut tx = db.begin().await?;
    tx.set_timeouts(Duration::from_mins(5), Duration::from_secs(30))
        .await?;
    crate::target::exec(
        &mut tx,
        "SET LOCAL idle_in_transaction_session_timeout = '10min'",
    )
    .await?;
    let mut cx = Ctx {
        src,
        tx,
        g,
        f: Fragment::default(),
        evaluated_at,
        rules: HashSet::new(),
        rule_ref: HashMap::new(),
        versions: HashMap::new(),
        cases: HashMap::new(),
        max_case_number: 0,
        honeypot_keys: HashMap::new(),
        actions: HashMap::new(),
        appeals: HashMap::new(),
        appeal_events: HashMap::new(),
        tickets: HashMap::new(),
        appeal_channel: None,
        ticket_entry_channel: None,
    };
    steps(&mut cx).await?;
    checks::run(&mut cx).await?;
    let Ctx { mut tx, f, g, .. } = cx;
    // A failed check (an invariant that didn't hold) rolls the server back
    // before its marker, so nothing of it is runnable and a repaired rerun
    // imports it.
    let failed: Vec<String> = f
        .checks
        .iter()
        .filter(|(_, c)| !c.ok)
        .map(|(name, c)| format!("check {name} failed: {}", c.failing.examples.join(", ")))
        .collect();
    if !failed.is_empty() {
        tx.rollback().await?;
        return Err(ImportError::Server(failed));
    }
    let guild = GuildId::try_from(g.id).map_err(|_| ImportError::server("invalid server ID"))?;
    match finish {
        Finish::Commit => {
            quack_db::import::mark_imported(&mut tx, guild, run, fingerprint).await?;
            tx.commit().await?;
        }
        Finish::Rollback => tx.rollback().await?,
    }
    tracing::info!(quack.guild_id = g.id, "server imported");
    Ok(f)
}

/// The tables in foreign-key order.
async fn steps(cx: &mut Ctx<'_>) -> Result<(), ImportError> {
    step(cx, "guilds", tables::guilds::import_guild).await?;
    step(cx, "rules", tables::rules::import).await?;
    step(cx, "guild_settings", tables::guilds::import_settings).await?;
    step(cx, "module_configurations", tables::modules::import).await?;
    step(cx, "cases", tables::cases::import).await?;
    step(cx, "case_action_executions", tables::actions::import).await?;
    step(cx, "case_notifications", tables::notifications::import).await?;
    step(cx, "case_evidence_snapshots", tables::evidence::import).await?;
    step(cx, "case_publications", tables::publications::import).await?;
    step(cx, "appeals", tables::appeals::import).await?;
    step(
        cx,
        "appeal_notifications",
        tables::appeal_notifications::import,
    )
    .await?;
    step(cx, "tickets", tables::tickets::import).await?;
    step(cx, "honeypot_triggers", tables::honeypot::import).await?;
    step(cx, "audit_log_entries", tables::audit::import).await?;
    let guild = GuildId::try_from(cx.g.id).map_err(|_| ImportError::server("invalid server ID"))?;
    quack_db::import::raise_case_counter(&mut cx.tx, guild, cx.max_case_number).await?;
    Ok(())
}

/// Runs one table group in its own span.
async fn step<F>(cx: &mut Ctx<'_>, table: &'static str, run: F) -> Result<(), ImportError>
where
    F: AsyncFnOnce(&mut Ctx<'_>) -> Result<(), ImportError>,
{
    let span = tracing::info_span!(
        "import table",
        import.table = table,
        quack.guild_id = cx.g.id
    );
    let before: u64 = cx.f.written.values().sum();
    run(cx).instrument(span).await.inspect_err(|e| {
        tracing::warn!(import.table = table, error = %e, "table import failed");
    })?;
    let after: u64 = cx.f.written.values().sum();
    tracing::info!(
        import.table = table,
        import.written = after - before,
        "table imported"
    );
    Ok(())
}
