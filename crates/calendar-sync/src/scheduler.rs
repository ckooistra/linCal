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
        // Passes requested by the user or a local change ignore account
        // backoff; timed ones respect it.
        let mut forced = true;
        loop {
            // Each pass runs as its own task: a panic ends that pass, not
            // the scheduler.
            let pass = {
                let engine = Arc::clone(&engine);
                tokio::spawn(async move { engine.sync_all(forced).await })
            };
            if pass.await.is_err() {
                eprintln!("[sync] a sync pass failed unexpectedly; continuing");
            }
            // Sleep until the interval elapses, a retry is due, or someone asks.
            let mut wait = interval;
            if let Ok(Some(due)) = engine.next_retry_ms().await {
                let ms = (due - Utc::now().timestamp_millis()).max(1000);
                wait = wait.min(Duration::from_millis(u64::try_from(ms).unwrap_or(1000)));
            }
            forced = tokio::select! {
                () = tokio::time::sleep(wait) => false,
                () = wake.notified() => {
                    tokio::time::sleep(DEBOUNCE).await;
                    true
                }
            };
        }
    };
    (handle, run)
}
