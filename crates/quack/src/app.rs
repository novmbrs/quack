//! Running one command: configuration, telemetry, the runtime, and exit
//! codes (0 success, 1 runtime failure, 2 usage or configuration error).

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use quack_telemetry::{LogFormat, TelemetryConfig, TelemetryGuard};

use crate::cli::{Cli, Command, USAGE};
use crate::config::{self, Config, Need};
use crate::serve::{self, DiscordMode, Options, Readiness};

/// The most time telemetry gets to flush at exit (§17.5 step 5).
const FLUSH_DEADLINE: Duration = Duration::from_secs(5);

/// Runs a parsed command line.
pub fn run(cli: Cli) -> ExitCode {
    let need = match &cli.command {
        Command::Version => {
            println!("quack {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Command::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Serve { .. } => Need::Serve,
        Command::Migrate => Need::Migrate,
        Command::ImportMysql { .. } => Need::Import,
        Command::CommandsSync => Need::CommandsSync,
    };
    let config = match config::load(need, cli.config.as_deref()) {
        Ok(config) => config,
        Err(problems) => {
            eprintln!("quack: the configuration has problems:\n{problems}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("quack: couldn't start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async move {
        let telemetry = match telemetry(&config) {
            Ok(guard) => guard,
            Err(error) => {
                eprintln!("quack: couldn't start telemetry: {error}");
                return ExitCode::FAILURE;
            }
        };
        let result = command(&cli.command, &config).await;
        let code = match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!("{error:#}");
                eprintln!("quack: {error:#}");
                ExitCode::FAILURE
            }
        };
        if let Err(error) =
            tokio::task::spawn_blocking(move || telemetry.with_deadline(FLUSH_DEADLINE).shutdown())
                .await
        {
            eprintln!("quack: couldn't flush telemetry: {error}");
        }
        code
    })
}

/// Telemetry from the configuration (§18.6).
fn telemetry(config: &Config) -> Result<TelemetryGuard, quack_telemetry::TelemetryError> {
    let mut t = TelemetryConfig::new(config.service_name.clone(), env!("CARGO_PKG_VERSION"));
    t.environment.clone_from(&config.environment);
    t.otlp_endpoint.clone_from(&config.otlp_endpoint);
    t.log_filter.clone_from(&config.log_level);
    t.log_format = if config.environment == "development" {
        LogFormat::Text
    } else {
        LogFormat::Json
    };
    quack_telemetry::init(&t)
}

async fn command(command: &Command, config: &Config) -> anyhow::Result<()> {
    match command {
        Command::Serve {
            migrate,
            discord_offline,
        } => {
            let discord = match discord_offline {
                Some(api_base) => {
                    tracing::warn!(%api_base, "serving without Discord (--discord-offline)");
                    DiscordMode::Off {
                        api_base: api_base.clone(),
                    }
                }
                None => DiscordMode::Connect,
            };
            let options = Options {
                migrate: *migrate,
                discord,
            };
            serve::run(
                config,
                &options,
                Arc::new(Readiness::default()),
                shutdown_signal(),
            )
            .await?;
            Ok(())
        }
        Command::Migrate => migrate(config).await,
        Command::CommandsSync => commands_sync(config).await,
        Command::ImportMysql { dry_run, report } => {
            crate::wiring::import_mysql(config, *dry_run, report.as_deref()).await
        }
        Command::Version | Command::Help => Ok(()),
    }
}

/// `quack migrate`: applies pending migrations and prints them.
async fn migrate(config: &Config) -> anyhow::Result<()> {
    let db = quack_db::Db::connect(config.database_url.expose(), 1).await?;
    let applied = quack_db::migrate(&db).await?;
    println!("The database has every migration this build knows:");
    for m in &applied {
        println!("  {} {}", m.version, m.description);
    }
    db.close().await;
    Ok(())
}

/// `quack commands sync` (§11.2).
async fn commands_sync(config: &Config) -> anyhow::Result<()> {
    let discord = config
        .discord
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the Discord token and application id are required"))?;
    let client = quack_discord::DiscordClient::new(&quack_discord::ClientConfig::new(
        discord.token.expose(),
        discord.application_id,
    ))?;
    let ids = serve::sync_commands(config, &client).await?;
    println!("Registered {} commands.", ids.0.len());
    Ok(())
}

/// Resolves on SIGTERM or SIGINT.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
