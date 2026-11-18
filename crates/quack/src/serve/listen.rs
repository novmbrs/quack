//! Step 8 of startup: the one `LISTEN` connection's notifications, routed
//! to the rules and config caches and, once it runs, the job runner.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use quack_db::jobs::Runner;
use quack_db::notify::{Listener, Notification};
use quack_engine::Engine;
use tokio_util::sync::CancellationToken;

/// Routes notifications until `stop`. `Rules`/`Settings` reload their
/// cache; `Job` and `Reconnected` wake the runner once it started (it
/// polls before that); `Reconnected` also reloads both caches.
pub(super) async fn route(
    mut listener: Listener,
    engine: Engine,
    runner: Arc<OnceLock<Runner>>,
    stop: CancellationToken,
) {
    loop {
        let received = tokio::select! {
            () = stop.cancelled() => return,
            n = listener.recv() => n,
        };
        let notification = match received {
            Ok(n) => n,
            Err(error) => {
                tracing::warn!(%error, "the notification listener couldn't reconnect; trying again");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if let Some(runner) = runner.get() {
            runner.on_notification(notification);
        }
        let reloaded = match notification {
            Notification::Rules(_) => {
                engine
                    .rules_cache()
                    .on_notification(engine.db(), notification)
                    .await
            }
            Notification::Settings(_) => {
                engine
                    .config_cache()
                    .on_notification(engine.db(), notification)
                    .await
            }
            Notification::Reconnected => {
                let rules = engine
                    .rules_cache()
                    .on_notification(engine.db(), notification)
                    .await;
                let config = engine
                    .config_cache()
                    .on_notification(engine.db(), notification)
                    .await;
                rules.and(config)
            }
            Notification::Job(_) => Ok(()),
        };
        if let Err(error) = reloaded {
            tracing::warn!(%error, "couldn't reload a cache after a notification");
        }
    }
}
