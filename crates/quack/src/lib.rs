//! The `quack` binary's pieces: configuration, the command line, the
//! `DiscordPort` adapter, and the `serve` startup and shutdown sequences.
//! One process per Discord application runs the gateway, interactions,
//! the HTTP API, the dashboard files, and the job workers (`quack serve`),
//! and hosts the operator commands (`migrate`, `import-mysql`,
//! `commands sync`).

pub mod app;
pub mod cli;
pub mod config;
pub mod port;
pub mod serve;
pub mod wiring;

pub use port::DiscordPort;
