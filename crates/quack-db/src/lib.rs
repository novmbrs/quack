//! Quack's PostgreSQL storage.
//!
//! Migrations, every query (sqlx, compile-time checked), transactions, the
//! job queue (`FOR UPDATE SKIP LOCKED` plus `LISTEN/NOTIFY`), sessions, and
//! tracked Discord messages. All durable state lives here so a restart loses
//! nothing.
