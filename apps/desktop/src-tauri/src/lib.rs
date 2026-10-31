//! Thin Tauri integration layer for Universal Calendar.
//!
//! This crate holds no calendar logic: commands in [`commands`] decode
//! arguments and forward to the Tauri-independent `calendar-storage` and
//! `calendar-core` crates. [`bench`] holds benchmark-only hooks.

mod bench;
mod commands;

use std::path::PathBuf;

use std::sync::Arc;
use std::time::Duration;

use calendar_storage::Store;
use calendar_sync::{
    CredentialStore, KeyringCredentials, MemoryCredentials, SyncEngine, default_factory,
};
use tauri::{Emitter, Manager};

use commands::{AppState, Sync};

/// How often remote accounts are checked when nothing else triggers a sync.
const SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Credential storage. `UC_CREDENTIALS=memory` keeps passwords in memory
/// only (lost on exit) and exists solely for testing on machines without
/// a Secret Service daemon.
fn credentials() -> Arc<dyn CredentialStore> {
    if std::env::var("UC_CREDENTIALS").is_ok_and(|v| v == "memory") {
        eprintln!("[credentials] UC_CREDENTIALS=memory: passwords are not saved (testing only)");
        Arc::new(MemoryCredentials::default())
    } else {
        Arc::new(KeyringCredentials::new("org.universalcalendar.desktop"))
    }
}

/// Where the database lives. Benchmark runs use a throw-away directory so
/// they never touch the user's calendar; `UC_DATA_DIR` overrides for testing.
fn data_dir(app: &tauri::App) -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("UC_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if bench::enabled() {
        return Ok(
            std::env::temp_dir().join(format!("universal-calendar-bench-{}", std::process::id()))
        );
    }
    app.path().app_data_dir().map_err(|e| e.to_string())
}

fn open_store(app: &tauri::App) -> Result<Store, String> {
    let dir = data_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("calendar.db");
    tauri::async_runtime::block_on(Store::open(&path)).map_err(|e| e.to_string())
}

/// Builds and runs the desktop application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let store = open_store(app);
            let sync = match &store {
                Ok(store) => {
                    let handle = app.handle().clone();
                    let engine = Arc::new(SyncEngine::new(
                        store.clone(),
                        credentials(),
                        default_factory(),
                        Arc::new(move |event| {
                            let _ = handle.emit("sync-event", event);
                        }),
                    ));
                    let (scheduler, run) =
                        calendar_sync::scheduler(Arc::clone(&engine), SYNC_INTERVAL);
                    if !bench::enabled() {
                        // Demo data from earlier versions; only benchmarks use it now.
                        if let Err(e) = tauri::async_runtime::block_on(store.remove_simulated()) {
                            eprintln!("[storage] could not remove simulated data: {e}");
                        }
                        // Somewhere to put events without any account.
                        if let Err(e) =
                            tauri::async_runtime::block_on(store.ensure_local_calendar())
                        {
                            eprintln!("[storage] could not create the local calendar: {e}");
                        }
                        tauri::async_runtime::spawn(run);
                    }
                    Some(Sync {
                        engine,
                        scheduler,
                        adding: std::sync::Mutex::new(None),
                    })
                }
                Err(reason) => {
                    eprintln!("[storage] could not open database: {reason}");
                    None
                }
            };
            app.manage(AppState { store, sync });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::list_calendars,
            commands::query_range,
            commands::search_events,
            commands::create_event,
            commands::update_event,
            commands::delete_event,
            commands::set_calendar_color,
            commands::get_settings,
            commands::set_setting,
            commands::load_simulated_dataset,
            commands::add_caldav_account,
            commands::add_ics_subscription,
            commands::cancel_add_account,
            commands::oauth_client_id,
            commands::add_google_account,
            commands::add_microsoft_account,
            commands::sign_in_again,
            commands::remove_account,
            commands::sync_now,
            commands::account_statuses,
            commands::resolve_conflict,
            commands::sync_problems,
            commands::resolve_sync_problem,
            bench::bench_config,
            bench::bench_memory,
            bench::bench_report
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Universal Calendar");
}
