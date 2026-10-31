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

use std::sync::atomic::{AtomicBool, Ordering};

use calendar_storage::Store;
use calendar_sync::{
    CredentialStore, DueReminder, KeyringCredentials, MemoryCredentials, SyncEngine, SyncEvent,
    default_factory,
};
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use commands::{AppState, NOTIFICATIONS_SETTING, Sync};

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

/// The system's time zone (for all-day and floating events), UTC if unknown.
fn system_zone() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// Shows a reminder as a desktop notification.
fn notify(app: &tauri::AppHandle, reminder: &DueReminder) {
    let tz = system_zone();
    let when = if reminder.all_day {
        "Today".to_owned()
    } else {
        let at = reminder.start.with_timezone(&tz).format("%H:%M");
        match reminder.minutes_before {
            0 => format!("Now, {at}"),
            m if m < 60 => format!("In {m} min, {at}"),
            m if m % 1440 == 0 => format!("In {} day(s), {at}", m / 1440),
            m => format!("In {} h {} min, {at}", m / 60, m % 60).replace(" 0 min", ""),
        }
    };
    let body = if reminder.location.is_empty() {
        when
    } else {
        format!("{when} · {}", reminder.location)
    };
    let title = if reminder.title.is_empty() {
        "(No title)"
    } else {
        &reminder.title
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[reminders] could not show a notification: {e}");
    }
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
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let store = open_store(app);
            let notifications = Arc::new(AtomicBool::new(true));
            // Reminders: shown while the app runs (not in benchmark runs).
            let reminders = match &store {
                Ok(store) if !bench::enabled() => {
                    if let Ok(settings) = tauri::async_runtime::block_on(store.settings()) {
                        let on = settings
                            .get(NOTIFICATIONS_SETTING)
                            .and_then(|v| v.as_bool());
                        notifications.store(on != Some(false), Ordering::Relaxed);
                    }
                    let handle = app.handle().clone();
                    let enabled = Arc::clone(&notifications);
                    let (reminders, run) =
                        calendar_sync::reminder_loop(store.clone(), system_zone, move |r| {
                            if enabled.load(Ordering::Relaxed) {
                                notify(&handle, &r);
                            }
                        });
                    tauri::async_runtime::spawn(run);
                    Some(reminders)
                }
                _ => None,
            };
            let sync = match &store {
                Ok(store) => {
                    let handle = app.handle().clone();
                    let wake = reminders.clone();
                    let engine = Arc::new(SyncEngine::new(
                        store.clone(),
                        credentials(),
                        default_factory(),
                        Arc::new(move |event| {
                            if let (SyncEvent::Synced { changed: true, .. }, Some(wake)) =
                                (&event, &wake)
                            {
                                wake.changed();
                            }
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
            app.manage(AppState {
                store,
                sync,
                reminders,
                notifications,
            });
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
            commands::conflict_versions,
            commands::resolve_sync_problem,
            bench::bench_config,
            bench::bench_memory,
            bench::bench_report
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Universal Calendar");
}
