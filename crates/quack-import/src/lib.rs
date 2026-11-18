//! One-time import from Quack's old MySQL database into PostgreSQL.
//!
//! Reads the live MySQL schema, converts it to the new model, and writes a
//! validation report. The only crate that enables sqlx's `mysql` feature.
