//! Spans for the module's use cases and jobs.

use std::future::Future;

use quack_core::ids::GuildId;
use quack_telemetry::spans;
use tracing::Instrument as _;

/// Runs `work` in a `use_case {name}` span for `guild`.
pub(crate) async fn span<T>(
    name: &'static str,
    guild: GuildId,
    work: impl Future<Output = T>,
) -> T {
    work.instrument(spans::use_case(name, Some(guild.get())))
        .await
}
