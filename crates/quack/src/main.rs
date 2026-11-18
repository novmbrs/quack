//! The `quack` binary: config, CLI, wiring, and graceful shutdown.
//!
//! One process per Discord application runs the gateway, interactions, the
//! HTTP API, the dashboard files, and the job workers (`quack serve`), and
//! also hosts the operator commands (`migrate`, `import-mysql`,
//! `commands sync`).

fn main() {
    println!("quack {}", env!("CARGO_PKG_VERSION"));
}
