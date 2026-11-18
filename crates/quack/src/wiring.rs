//! **Integration points** for the crates that are still being built: the
//! optional modules, the HTTP API, and the MySQL importer. Each function
//! here is where one of them is connected; until then each returns the
//! "nothing connected" value and `serve` runs without it.

use std::sync::Arc;

use quack_discord::gateway::Event;
use quack_engine::Engine;
use quack_engine::jobs::ModuleJobs;

use crate::config::Config;
use crate::serve::Readiness;

/// The optional modules for the bot's router (§13.1.6).
///
/// Connect: `vec![adapter(quack_tickets::module()),
/// adapter(quack_activity_log::module()), adapter(quack_honeypot::module())]`,
/// where each adapter implements [`quack_bot::Module`] by converting the
/// gateway `Event` and the component call into the engine's
/// `ModuleEvent`/`ComponentCall` and calling the module's `on_event`,
/// `on_component`, `setup_panel`/`on_setup`.
pub fn modules(_engine: &Engine) -> Vec<Arc<dyn quack_bot::Module>> {
    Vec::new()
}

/// The modules' job kinds and handlers for the job runner (§16.4).
///
/// Connect: one [`ModuleJobs`] per module, `kinds` from its spec's
/// `job_kinds` and `handler` calling its `run_job` (`Phase::Run`) or
/// `finalize_job` (`Phase::Finalize`).
pub fn module_jobs(_engine: &Engine) -> Vec<ModuleJobs> {
    Vec::new()
}

/// Runs in the shard loop for each event a full queue dropped (§11.9):
/// a dropped event in a ticket thread sets that ticket's `journal_gaps`.
///
/// Connect: `quack_tickets`'s "journal gap" hook, kept cheap (it must not
/// block the shard loop; queue the write).
pub fn on_dropped_event(_event: &Event) {}

/// The HTTP API's routes, merged with `/livez` and `/readyz`.
///
/// Connect: `quack_api::router(ApiState::new(engine, api config from
/// `config`, …).with_modules(…))`, and give the API the readiness facts by
/// implementing `quack_api::ReadinessFacts` for [`Readiness`] (or passing
/// its flags). The API then serves `/livez` and `/readyz` itself, and
/// `crate::serve::http::health` is dropped.
pub fn api_router(_engine: &Engine, _config: &Config, _ready: &Arc<Readiness>) -> axum::Router {
    axum::Router::new()
}

/// `quack import-mysql` (§19).
#[allow(clippy::unused_async, reason = "the importer it connects to is async")]
///
/// Connect: `quack_import::run(ImportOptions { source: config
/// .import_source_url, target: db, dry_run, report })` and print its
/// `Report`; return an error when validation failed.
pub async fn import_mysql(
    _config: &Config,
    _dry_run: bool,
    _report: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("the MySQL importer is not connected to this build yet")
}
