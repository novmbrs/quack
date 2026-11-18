//! Typed metric and span attributes.
//!
//! Metric attributes are either a closed enum (every value is listed here) or
//! a static name (a `&'static str` chosen in code, such as a command or route
//! template). Neither can carry runtime data, so a call site cannot put a
//! guild, user, channel, case, or message id on a metric. Ids belong on spans
//! and logs.

use opentelemetry::KeyValue;

/// A typed attribute: its key and the value exported for it.
pub trait Attribute: Copy {
    /// The attribute key every value of this type is exported under.
    const KEY: &'static str;
    /// Every allowed value; empty for static names chosen in code.
    const VALUES: &'static [&'static str] = &[];

    /// The exported value.
    fn as_str(self) -> &'static str;

    /// This value as an OpenTelemetry key-value pair.
    fn kv(self) -> KeyValue {
        KeyValue::new(Self::KEY, self.as_str())
    }
}

/// Defines a closed attribute enum with its key and exported values.
macro_rules! label_enum {
    (
        $(#[$meta:meta])*
        $name:ident = $key:literal {
            $($(#[$vmeta:meta])* $variant:ident => $value:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }

        impl Attribute for $name {
            const KEY: &'static str = $key;
            const VALUES: &'static [&'static str] = &[$($value,)+];

            fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

/// Defines an open-but-bounded attribute whose values are static names.
macro_rules! static_label {
    ($(#[$meta:meta])* $name:ident = $key:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(&'static str);

        impl $name {
            /// Wraps a name chosen in code. It must be a fixed, low-cardinality
            /// value, never built from runtime data.
            pub const fn new(name: &'static str) -> Self {
                Self(name)
            }

            /// The value, without importing [`Attribute`].
            pub const fn as_str(self) -> &'static str {
                self.0
            }
        }

        impl Attribute for $name {
            const KEY: &'static str = $key;

            fn as_str(self) -> &'static str {
                self.0
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.0)
            }
        }
    };
}

label_enum! {
    /// The kind of Discord interaction being handled.
    InteractionKind = "quack.interaction.kind" {
        /// A slash command.
        SlashCommand => "slash_command",
        /// A user context-menu command.
        UserCommand => "user_command",
        /// A message context-menu command.
        MessageCommand => "message_command",
        /// A button or select menu.
        Component => "component",
        /// A submitted modal.
        Modal => "modal",
        /// An autocomplete request.
        Autocomplete => "autocomplete",
    }
}

label_enum! {
    /// How handling an interaction ended.
    InteractionOutcome = "quack.interaction.outcome" {
        /// Quack did what was asked.
        Done => "done",
        /// Quack said no for a reason the person can act on (permissions,
        /// validation, a voided case).
        Refused => "refused",
        /// Something went wrong on Quack's side.
        Failed => "failed",
    }
}

label_enum! {
    /// The terminal outcome of a piece of work (glossary: outcome).
    Outcome = "quack.outcome" {
        /// It happened.
        Done => "done",
        /// It definitely did not happen.
        Failed => "failed",
        /// Quack couldn't confirm whether it happened.
        Unknown => "unknown",
        /// It was not needed or deliberately left alone.
        Skipped => "skipped",
    }
}

label_enum! {
    /// Why something failed or was refused.
    Reason = "quack.reason" {
        /// Quack lacks a Discord permission.
        MissingPermission => "missing_permission",
        /// The member's top role is at or above Quack's or the moderator's.
        RoleHierarchy => "role_hierarchy",
        /// The member owns the server.
        MemberIsOwner => "member_is_owner",
        /// The member is not in the server.
        MemberNotFound => "member_not_found",
        /// The member does not accept DMs.
        DmsClosed => "dms_closed",
        /// Quack and the member share no server.
        NoSharedServer => "no_shared_server",
        /// The person running the command lacks the staff role it needs.
        NotAllowed => "not_allowed",
        /// The input failed validation.
        Invalid => "invalid",
        /// The thing it applies to no longer allows it (for example a voided case).
        Conflict => "conflict",
        /// Discord rate-limited the request.
        RateLimited => "rate_limited",
        /// A call took too long.
        Timeout => "timeout",
        /// Discord returned an unexpected error.
        Discord => "discord",
        /// The database returned an error.
        Database => "database",
        /// Every allowed retry was used up.
        RetriesExhausted => "retries_exhausted",
        /// A bug or unexpected state in Quack.
        Internal => "internal",
    }
}

label_enum! {
    /// What Quack does on Discord for a case, including the undo actions.
    ActionKind = "quack.action.kind" {
        /// Timeout the member.
        Timeout => "timeout",
        /// Kick the member.
        Kick => "kick",
        /// Ban the member.
        Ban => "ban",
        /// Remove a timeout.
        RemoveTimeout => "remove_timeout",
        /// Unban the member.
        Unban => "unban",
    }
}

label_enum! {
    /// Where a case was created. The interaction kind stays on the
    /// interaction instruments.
    CaseSource = "quack.case.source" {
        /// `/case add` or an "Add case" command.
        Discord => "discord",
        /// The dashboard.
        Dashboard => "dashboard",
        /// The honeypot module.
        Honeypot => "honeypot",
    }
}

label_enum! {
    /// How one action attempt ended (`attempt_outcome`).
    AttemptOutcome = "quack.action.attempt.outcome" {
        /// Discord confirmed the change.
        Done => "done",
        /// The desired state already held, or an undo found Discord changed
        /// outside the case.
        NoChange => "no_change",
        /// Discord definitely refused.
        Refused => "refused",
        /// Nothing reached Discord (a read failed, or a rate limit came
        /// first).
        NotSent => "not_sent",
        /// The change call's result is unknown.
        Unknown => "unknown",
        /// The next run found the attempt still open.
        Interrupted => "interrupted",
    }
}

label_enum! {
    /// The terminal state an action reached.
    ActionState = "quack.action.state" {
        /// Done (including no change).
        Done => "done",
        /// Failed.
        Failed => "failed",
        /// An original that will not run because its case was voided.
        Cancelled => "cancelled",
        /// An undo that left Discord unchanged.
        Skipped => "skipped",
    }
}

label_enum! {
    /// Why a case was voided.
    VoidCause = "quack.void.cause" {
        /// Staff voided it directly.
        Staff => "staff",
        /// Staff accepted the member's appeal.
        AppealAccepted => "appeal_accepted",
    }
}

label_enum! {
    /// What a DM is about.
    DmKind = "quack.dm.kind" {
        /// The one DM about a new case.
        Case => "case",
        /// A DM about a staff decision on an appeal, or Quack closing it.
        AppealUpdate => "appeal_update",
        /// The DM with a closed ticket's transcript.
        TicketClosed => "ticket_closed",
    }
}

label_enum! {
    /// The terminal state of a DM.
    DmOutcome = "quack.dm.outcome" {
        /// Discord accepted the message.
        Sent => "sent",
        /// Discord definitely refused it.
        Failed => "failed",
        /// Quack couldn't confirm whether it was sent.
        Unknown => "unknown",
        /// No DM was needed.
        NotNeeded => "not_needed",
    }
}

label_enum! {
    /// A change to an appeal.
    AppealTransition = "quack.appeal.transition" {
        /// The member submitted it.
        Submit => "submit",
        /// Staff accepted it.
        Accept => "accept",
        /// Staff rejected it.
        Reject => "reject",
        /// Staff asked the member for more information.
        RequestInfo => "request_info",
        /// The member answered a request for more information.
        Reply => "reply",
        /// It was closed without a decision.
        Close => "close",
        /// Staff reopened it.
        Reopen => "reopen",
    }
}

label_enum! {
    /// A Discord message Quack keeps up to date (`message_kind`).
    MessageKind = "quack.message.kind" {
        /// The receipt posted for a case.
        CaseReceipt => "case_receipt",
        /// The staff post for an appeal.
        AppealPost => "appeal_post",
        /// A tickets panel.
        TicketPanel => "ticket_panel",
        /// The staff post for a ticket.
        TicketPost => "ticket_post",
        /// The honeypot channel warning.
        HoneypotWarning => "honeypot_warning",
    }
}

label_enum! {
    /// The result a moderation-log entry records (`mod_log_result`).
    ModLogResult = "quack.modlog.result" {
        /// It happened.
        Ok => "ok",
        /// Quack tried and it didn't work.
        Failed => "failed",
        /// Quack's access stopped it.
        Refused => "refused",
    }
}

label_enum! {
    /// A kind of background job.
    JobKind = "quack.job.kind" {
        /// Carry out a case's action or an undo action.
        RunAction => "run_action",
        /// Send a DM (case, appeal update, or closed ticket).
        SendDm => "send_dm",
        /// Bring a tracked Discord message up to date.
        SyncMessage => "sync_message",
        /// Post a moderation-log entry to the log channel.
        PostModLog => "post_mod_log",
        /// Save evidence for a case.
        CaptureEvidence => "capture_evidence",
        /// Add a case for a member's honeypot posts.
        HoneypotCase => "honeypot_case",
        /// Delete what a honeypot visitor posted.
        HoneypotCleanup => "honeypot_cleanup",
        /// Create a ticket's thread and welcome the member.
        OpenTicket => "open_ticket",
        /// Freeze a ticket, save its transcript, and close it.
        CloseTicket => "close_ticket",
        /// Expire sessions, drafts, and operation keys.
        Housekeeping => "housekeeping",
    }
}

label_enum! {
    /// How one run of a job ended.
    JobOutcome = "quack.job.outcome" {
        /// The job finished.
        Done => "done",
        /// The run failed and the job is scheduled again (an attempt used).
        Retry => "retry",
        /// The job waits without using an attempt (member busy, waiting
        /// for a DM).
        Deferred => "deferred",
        /// Its time slice ended with work left; it continues at once.
        Continued => "continued",
        /// It is not due yet and runs again at a later time.
        Rescheduled => "rescheduled",
        /// Its budget was spent and its finalization settled it.
        Finalized => "finalized",
        /// The run failed and the job will not run again.
        Dead => "dead",
        /// The run's lease expired, so another worker owns the job now.
        LostLease => "lost_lease",
        /// The run exceeded its handler deadline.
        TimedOut => "timed_out",
    }
}

label_enum! {
    /// The outcome class of a Discord REST call.
    RestOutcome = "quack.discord.outcome" {
        /// Discord did it.
        Done => "done",
        /// Discord definitely said no.
        Refused => "refused",
        /// Quack couldn't confirm what happened (timeout, connection loss, 5xx).
        Unknown => "unknown",
        /// Discord asked Quack to retry later.
        RateLimited => "rate_limited",
        /// The request never left Quack (connecting failed, shutting down,
        /// or a request Quack refused to build).
        NotSent => "not_sent",
    }
}

label_enum! {
    /// Which Discord rate limit was hit.
    RateLimitScope = "quack.discord.rate_limit.scope" {
        /// A per-route bucket.
        Route => "route",
        /// The bot-wide global limit.
        Global => "global",
        /// A resource-wide shared bucket.
        Shared => "shared",
    }
}

label_enum! {
    /// A change in the gateway connection.
    GatewayConnectionEvent = "quack.gateway.connection.event" {
        /// A new session was identified.
        Identify => "identify",
        /// A session was resumed.
        Resume => "resume",
        /// The connection dropped and is reconnecting.
        Reconnect => "reconnect",
        /// Discord invalidated the session.
        InvalidSession => "invalid_session",
    }
}

label_enum! {
    /// An HTTP request method (semconv `http.request.method`).
    HttpMethod = "http.request.method" {
        /// `GET`.
        Get => "GET",
        /// `HEAD`.
        Head => "HEAD",
        /// `POST`.
        Post => "POST",
        /// `PUT`.
        Put => "PUT",
        /// `PATCH`.
        Patch => "PATCH",
        /// `DELETE`.
        Delete => "DELETE",
        /// `OPTIONS`.
        Options => "OPTIONS",
        /// Any other method.
        Other => "_OTHER",
    }
}

label_enum! {
    /// How a dashboard sign-in ended.
    SignInOutcome = "quack.sign_in.outcome" {
        /// The person is signed in.
        Done => "done",
        /// The person cancelled on Discord.
        Cancelled => "cancelled",
        /// Quack refused (bad state, missing 2FA where required).
        Refused => "refused",
        /// Discord or Quack failed.
        Failed => "failed",
    }
}

label_enum! {
    /// A database operation (semconv `db.operation.name`).
    DbOperation = "db.operation.name" {
        /// `SELECT`.
        Select => "SELECT",
        /// `INSERT`.
        Insert => "INSERT",
        /// `UPDATE`.
        Update => "UPDATE",
        /// `DELETE`.
        Delete => "DELETE",
        /// `LISTEN` or `NOTIFY`.
        Notify => "NOTIFY",
        /// A migration or other DDL.
        Migrate => "MIGRATE",
    }
}

label_enum! {
    /// A class of database error (semconv `error.type`).
    DbErrorClass = "error.type" {
        /// `40001`: serialization failure.
        SerializationFailure => "serialization_failure",
        /// `40P01`: deadlock detected.
        Deadlock => "deadlock",
        /// `23505`: unique violation.
        UniqueViolation => "unique_violation",
        /// `23503`: foreign key violation.
        ForeignKeyViolation => "foreign_key_violation",
        /// `23514`: check violation.
        CheckViolation => "check_violation",
        /// The connection failed or was lost.
        Connection => "connection",
        /// A statement or lock timeout.
        Timeout => "timeout",
        /// No connection was free in time.
        PoolTimeout => "pool_timeout",
        /// A row Quack couldn't read back (a value its types don't allow).
        Decode => "decode",
        /// Any other error.
        Other => "_OTHER",
    }
}

label_enum! {
    /// How a database transaction ended.
    TransactionOutcome = "quack.db.transaction.outcome" {
        /// Committed.
        Commit => "commit",
        /// Rolled back.
        Rollback => "rollback",
    }
}

label_enum! {
    /// A database pool connection state (semconv `db.client.connection.state`).
    ConnectionState = "db.client.connection.state" {
        /// Idle in the pool.
        Idle => "idle",
        /// Checked out.
        Used => "used",
    }
}

label_enum! {
    /// The result of a cache lookup.
    CacheResult = "quack.cache.result" {
        /// The value was cached.
        Hit => "hit",
        /// The value was not cached.
        Miss => "miss",
    }
}

label_enum! {
    /// How far a gateway event got through the server activity log.
    ActivityStage = "quack.activity.stage" {
        /// The module saw the event.
        Seen => "seen",
        /// The module posted it to the log channel.
        Posted => "posted",
        /// The module dropped it (queue full, route gone).
        Dropped => "dropped",
        /// Discord refused the post, or it ran out of tries.
        Failed => "failed",
    }
}

label_enum! {
    /// What happened when someone posted in a honeypot channel.
    HoneypotOutcome = "quack.honeypot.outcome" {
        /// Quack created a case for them.
        Actioned => "actioned",
        /// They are staff or exempt, so Quack left them alone.
        Exempt => "exempt",
        /// Quack couldn't act on them.
        Failed => "failed",
    }
}

label_enum! {
    /// A CPU mode (semconv `cpu.mode`).
    CpuMode = "cpu.mode" {
        /// Time in user space.
        User => "user",
        /// Time in the kernel.
        System => "system",
    }
}

static_label! {
    /// An application command name, such as `case add` or `Add case`.
    CommandName = "quack.command"
}

static_label! {
    /// An API route template (semconv `http.route`), such as `/api/guilds/{guild_id}/cases`.
    HttpRoute = "http.route"
}

static_label! {
    /// A Discord REST route template (semconv `url.template`), such as
    /// `/guilds/{guild.id}/bans/{user.id}`.
    UrlTemplate = "url.template"
}

static_label! {
    /// A gateway event type, such as `MESSAGE_CREATE`.
    GatewayEvent = "quack.gateway.event"
}

static_label! {
    /// A moderation-log entry kind, such as `case_created`.
    ModLogKind = "quack.modlog.kind"
}

static_label! {
    /// A database table (semconv `db.collection.name`).
    DbCollection = "db.collection.name"
}

static_label! {
    /// A database pool name (semconv `db.client.connection.pool.name`).
    PoolName = "db.client.connection.pool.name"
}

static_label! {
    /// A bounded in-memory cache, such as `rules` or `activity_log.messages`.
    CacheName = "quack.cache.name"
}

static_label! {
    /// A bounded in-memory queue, such as `gateway.events`.
    QueueName = "quack.queue.name"
}

static_label! {
    /// An API rate-limit class, such as `sign_in` or `write`.
    RateLimitClass = "quack.rate_limit.class"
}

static_label! {
    /// A server activity log event type, such as `message_delete`.
    ActivityEvent = "quack.activity.event"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn assert_unique(values: &[&str]) {
        let unique: HashSet<_> = values.iter().collect();
        assert_eq!(unique.len(), values.len(), "duplicate value in {values:?}");
    }

    #[test]
    fn enum_values_are_unique_and_match_as_str() {
        assert_unique(Outcome::VALUES);
        assert_unique(Reason::VALUES);
        assert_unique(ActionKind::VALUES);
        assert_unique(JobKind::VALUES);
        assert_unique(JobOutcome::VALUES);
        assert_unique(AttemptOutcome::VALUES);
        assert_unique(HttpMethod::VALUES);
        for (kind, value) in JobKind::ALL.iter().zip(JobKind::VALUES) {
            assert_eq!(kind.as_str(), *value);
        }
    }

    #[test]
    fn values_named_by_the_design() {
        assert_eq!(
            JobKind::VALUES,
            [
                "run_action",
                "send_dm",
                "sync_message",
                "post_mod_log",
                "capture_evidence",
                "honeypot_case",
                "honeypot_cleanup",
                "open_ticket",
                "close_ticket",
                "housekeeping"
            ]
        );
        assert_eq!(
            JobOutcome::VALUES,
            [
                "done",
                "retry",
                "deferred",
                "continued",
                "rescheduled",
                "finalized",
                "dead",
                "lost_lease",
                "timed_out"
            ]
        );
        assert_eq!(CaseSource::VALUES, ["discord", "dashboard", "honeypot"]);
        assert_eq!(DmKind::VALUES, ["case", "appeal_update", "ticket_closed"]);
        assert_eq!(
            MessageKind::VALUES,
            [
                "case_receipt",
                "appeal_post",
                "ticket_panel",
                "ticket_post",
                "honeypot_warning"
            ]
        );
        assert_eq!(
            ActivityStage::VALUES,
            ["seen", "posted", "dropped", "failed"]
        );
        assert_eq!(
            AttemptOutcome::VALUES,
            [
                "done",
                "no_change",
                "refused",
                "not_sent",
                "unknown",
                "interrupted"
            ]
        );
        assert_eq!(ModLogResult::VALUES, ["ok", "failed", "refused"]);
        assert_eq!(
            ActionState::VALUES,
            ["done", "failed", "cancelled", "skipped"]
        );
        assert_eq!(AttemptOutcome::KEY, "quack.action.attempt.outcome");
        assert_eq!(ModLogResult::KEY, "quack.modlog.result");
        assert_eq!(ActionState::KEY, "quack.action.state");
    }

    #[test]
    fn kv_uses_the_type_key() {
        let kv = ActionKind::Ban.kv();
        assert_eq!(kv.key.as_str(), "quack.action.kind");
        assert_eq!(kv.value.as_str(), "ban");
        assert_eq!(CommandName::new("case add").kv().value.as_str(), "case add");
    }
}
