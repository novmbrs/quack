//! The crate's one error type and how database errors are classified.

use quack_telemetry::labels::DbErrorClass;

/// A unique constraint that rejected a write, by what it means in the domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The operation key was already used (`operation_keys` or a case's
    /// stored operation key).
    OperationKeyUsed,
    /// Another active rule in the server has the same name.
    RuleNameTaken,
    /// The case number already exists (a counter bug; never expected).
    CaseNumberTaken,
    /// The voided case already has a replacement.
    CaseAlreadyReplaced,
    /// The case already has its original action, or the original already
    /// has its undo.
    ActionExists,
    /// The action already has an open attempt (an attempt number that
    /// already exists is [`Conflict::AlreadyExists`]).
    AttemptOpen,
    /// The case already has evidence for this message.
    EvidenceExists,
    /// The case already has an appeal.
    AppealExists,
    /// The DM already exists (one per case, appeal event, or ticket).
    DmExists,
    /// The owner already has its tracked message.
    TrackedMessageExists,
    /// The member already has a live ticket.
    LiveTicketExists,
    /// Another ticket already uses this thread.
    ThreadTaken,
    /// A row with this primary key already exists.
    AlreadyExists,
    /// Another unique constraint.
    Other,
}

impl Conflict {
    /// Maps a constraint or unique index name from the schema.
    pub fn from_constraint(name: &str) -> Self {
        match name {
            "operation_keys_pkey" | "cases_guild_id_operation_key_key" => Self::OperationKeyUsed,
            "rules_active_name" => Self::RuleNameTaken,
            "cases_guild_id_number_key" => Self::CaseNumberTaken,
            "cases_replaces" => Self::CaseAlreadyReplaced,
            "actions_one_original" | "actions_undo_of_action_id_key" => Self::ActionExists,
            "action_attempts_one_open" => Self::AttemptOpen,
            "evidence_case_id_message_id_key" => Self::EvidenceExists,
            "appeals_case_id_key" => Self::AppealExists,
            "dms_one_per_case" | "dms_appeal_event_id_key" | "dms_ticket_id_key" => Self::DmExists,
            "messages_case" | "messages_appeal" | "messages_ticket" | "messages_singleton" => {
                Self::TrackedMessageExists
            }
            "tickets_one_live" => Self::LiveTicketExists,
            "tickets_thread_id_key" => Self::ThreadTaken,
            name if name.ends_with("_pkey") => Self::AlreadyExists,
            _ => Self::Other,
        }
    }
}

/// A rule of the schema other than uniqueness that rejected a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A `CHECK` constraint.
    Check,
    /// A foreign key: a referenced row is missing or belongs to another
    /// server, case, or member.
    ForeignKey,
    /// A `NOT NULL` column.
    NotNull,
    /// An append-only table (`mod_log`, `rule_versions`) refused an update,
    /// delete, or truncate.
    AppendOnly,
}

/// Every failure of this crate.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// A row the operation requires does not exist.
    #[error("not found")]
    NotFound,
    /// A unique constraint rejected the write.
    #[error("conflict ({0:?}) on {1}")]
    Conflict(Conflict, String),
    /// A compare-and-set found the row in another state than expected, or a
    /// job's claim no longer holds. The caller's transaction must roll back.
    #[error("the row changed since it was read")]
    Stale,
    /// A serialization failure or deadlock: the whole transaction may be run
    /// again (see [`Db::transaction`](crate::Db::transaction)).
    #[error("transaction conflict ({0:?})")]
    Contention(DbErrorClass, #[source] sqlx::Error),
    /// The database is unreachable, the pool had no free connection in time,
    /// or a statement or lock timed out.
    #[error("database unavailable ({0:?})")]
    Unavailable(DbErrorClass, #[source] sqlx::Error),
    /// A `CHECK`, foreign key, `NOT NULL`, or append-only rule rejected the
    /// write.
    #[error("rejected by {constraint} ({violation:?})")]
    Rejected {
        /// What kind of rule.
        violation: Violation,
        /// The constraint name (or table, for append-only), never a value.
        constraint: String,
    },
    /// A value given to this crate can't be stored (out of range).
    #[error("invalid value: {0}")]
    Invalid(String),
    /// A stored value could not be read into its domain type.
    #[error("unreadable value: {0}")]
    Decode(String),
    /// Applying or checking migrations failed.
    #[error("migration failed")]
    Migrate(#[source] sqlx::migrate::MigrateError),
    /// Any other database error.
    #[error("database error")]
    Other(#[source] sqlx::Error),
}

impl DbError {
    /// The `error.type` class for spans and metrics.
    pub fn class(&self) -> DbErrorClass {
        match self {
            Self::Conflict(..) => DbErrorClass::UniqueViolation,
            Self::Contention(class, _) | Self::Unavailable(class, _) => *class,
            Self::Rejected {
                violation: Violation::ForeignKey,
                ..
            } => DbErrorClass::ForeignKeyViolation,
            Self::Rejected {
                violation: Violation::Check,
                ..
            } => DbErrorClass::CheckViolation,
            Self::Decode(_) => DbErrorClass::Decode,
            _ => DbErrorClass::Other,
        }
    }

    /// Whether running the same transaction again may succeed
    /// (serialization failure or deadlock).
    pub fn is_contention(&self) -> bool {
        matches!(self, Self::Contention(..))
    }

    /// The conflict, when a unique constraint rejected the write.
    pub fn conflict(&self) -> Option<Conflict> {
        match self {
            Self::Conflict(conflict, _) => Some(*conflict),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        classify(error)
    }
}

impl From<sqlx::migrate::MigrateError> for DbError {
    fn from(error: sqlx::migrate::MigrateError) -> Self {
        Self::Migrate(error)
    }
}

/// Sorts a sqlx error into a [`DbError`] by its SQLSTATE or kind.
fn classify(error: sqlx::Error) -> DbError {
    match &error {
        sqlx::Error::RowNotFound => DbError::NotFound,
        sqlx::Error::PoolTimedOut => DbError::Unavailable(DbErrorClass::PoolTimeout, error),
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::Protocol(_) => DbError::Unavailable(DbErrorClass::Connection, error),
        sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => {
            DbError::Decode("column".to_owned())
        }
        sqlx::Error::Database(db) => {
            let code = db.code().unwrap_or_default().into_owned();
            let constraint = db.constraint().map(str::to_owned);
            let table = db.table().map(str::to_owned);
            match code.as_str() {
                "23505" => {
                    let name = constraint.unwrap_or_default();
                    DbError::Conflict(Conflict::from_constraint(&name), name)
                }
                "23503" => rejected(Violation::ForeignKey, constraint),
                "23514" => rejected(Violation::Check, constraint),
                "23502" => rejected(Violation::NotNull, constraint.or(table)),
                // The append-only trigger raises 42501 with "<table> is
                // append-only"; any other 42501 is a real permission error.
                "42501" => match db.message().strip_suffix(" is append-only") {
                    Some(table) => rejected(Violation::AppendOnly, Some(table.to_owned())),
                    None => DbError::Other(error),
                },
                "40001" => DbError::Contention(DbErrorClass::SerializationFailure, error),
                "40P01" => DbError::Contention(DbErrorClass::Deadlock, error),
                // query_canceled (statement_timeout) and lock_not_available.
                "57014" | "55P03" => DbError::Unavailable(DbErrorClass::Timeout, error),
                // Connection exceptions, admin shutdown, cannot connect now.
                c if c.starts_with("08") || c == "57P01" || c == "57P03" => {
                    DbError::Unavailable(DbErrorClass::Connection, error)
                }
                _ => DbError::Other(error),
            }
        }
        _ => DbError::Other(error),
    }
}

fn rejected(violation: Violation, constraint: Option<String>) -> DbError {
    DbError::Rejected {
        violation,
        constraint: constraint.unwrap_or_default(),
    }
}

/// Turns a compare-and-set's "applied" flag into a result, for callers that
/// must roll back when it did not apply.
pub trait Applied {
    /// `Ok(())` when the write applied, else [`DbError::Stale`].
    fn or_stale(self) -> Result<(), DbError>;
}

impl Applied for bool {
    fn or_stale(self) -> Result<(), DbError> {
        if self { Ok(()) } else { Err(DbError::Stale) }
    }
}
