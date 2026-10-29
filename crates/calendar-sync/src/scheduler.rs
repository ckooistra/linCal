//! Runs sync passes in the background without polling: on start-up, on
//! request (local change, "Sync now"), at a modest interval, and when a
//! queued retry becomes due.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Notify;

use crate::engine::SyncEngine;

/// Bursts of local edits are coalesced into one pass.
const DEBOUNCE: Duration = Duration::from_millis(1500);

#[derive(Clone)]
pub struct SchedulerHandle {
    wake: Arc<Notify>,
}

impl SchedulerHandle {
    /// Requests a sync pass soon (coalesced with other requests).
    pub fn trigger(&self) {
        self.wake.notify_one();
    }
}

/// Creates the scheduler: a handle for triggering passes and the loop
/// future, which the caller spawns on its runtime.
pub fn scheduler(
    engine: Arc<SyncEngine>,
    interval: Duration,
) -> (
    SchedulerHandle,
    impl std::future::Future<Output = ()> + Send,
) {
    let wake = Arc::new(Notify::new());
    let handle = SchedulerHandle {
        wake: Arc::clone(&wake),
    };
    let run = async move {
        loop {
            let _ = engine.sync_all().await;
            // Sleep until the interval elapses, a retry is due, or someone asks.
            let mut wait = interval;
            if let Ok(Some(due)) = engine.next_retry_ms().await {
                let ms = (due - Utc::now().timestamp_millis()).max(1000);
                wait = wait.min(Duration::from_millis(u64::try_from(ms).unwrap_or(1000)));
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wake.notified() => {
                    tokio::time::sleep(DEBOUNCE).await;
                }
            }
        }
    };
    (handle, run)
}
