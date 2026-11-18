//! The gauge sampler's queries run against a server with failed actions
//! and open appeals.

use anyhow::Result;
use quack_core::rule::LevelAction;
use quack_engine::gauges::Gauges;
use quack_engine::port::RefusalKind;
use quack_testkit::fake::{Call, Script};
use quack_testkit::rules_for_tests::one;
use quack_testkit::{Harness, MEMBER};

#[tokio::test]
async fn sampling_reads_the_failed_actions_and_the_backlog() -> Result<()> {
    let h = Harness::new().await?;
    let rule = h.rule(one("Raid", LevelAction::Kick, false)).await?;
    h.fake
        .script(Call::Kick, Script::refuse(RefusalKind::MissingPermissions));
    h.apply(MEMBER, rule).await?;
    let gauges = Gauges::install();
    gauges.sample(&h.db.db).await?;
    h.settle().await?;
    gauges.sample(&h.db.db).await?;
    Ok(())
}
