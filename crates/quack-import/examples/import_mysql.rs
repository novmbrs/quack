//! Runs `quack import-mysql` without the `quack` binary.
//!
//! ```text
//! QUACK_IMPORT_SOURCE_URL=mysql://user:pass@host:3306/quack \
//! QUACK_DATABASE_URL=postgres://user:pass@host:5432/quack \
//! cargo run -p quack-import --example import_mysql -- [--dry-run] [--report report.json] [--migrate]
//! ```
//!
//! `--migrate` applies the PostgreSQL migrations first (for a fresh
//! throwaway database). Prints the human summary and exits 0 when the
//! report is OK, 1 otherwise.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use quack_import::ImportOptions;

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let mut options = ImportOptions {
        source_url: std::env::var("QUACK_IMPORT_SOURCE_URL")
            .context("set QUACK_IMPORT_SOURCE_URL")?,
        dry_run: false,
        report_path: None,
    };
    let target = std::env::var("QUACK_DATABASE_URL").context("set QUACK_DATABASE_URL")?;
    let mut migrate = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => options.dry_run = true,
            "--migrate" => migrate = true,
            "--report" => {
                options.report_path =
                    Some(PathBuf::from(args.next().context("--report needs a path")?));
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let mut telemetry =
        quack_telemetry::TelemetryConfig::new("quack-import", env!("CARGO_PKG_VERSION"));
    telemetry.otlp_endpoint = std::env::var("QUACK_OTLP_ENDPOINT").ok();
    if let Ok(filter) = std::env::var("QUACK_LOG") {
        telemetry.log_filter = filter;
    }
    let _telemetry = quack_telemetry::init(&telemetry)?;
    let db = quack_db::Db::connect(&target, 4).await?;
    if migrate {
        quack_db::migrate(&db).await?;
    }
    let report = quack_import::run(&db, &options).await?;
    println!("{}", report.summary());
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
