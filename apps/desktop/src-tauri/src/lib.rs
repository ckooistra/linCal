//! Thin Tauri integration layer for Universal Calendar.
//!
//! This crate holds no calendar logic: commands in [`commands`] decode
//! arguments and forward to the Tauri-independent `calendar-storage` and
//! `calendar-core` crates. [`bench`] holds benchmark-only hooks.

mod bench;
mod commands;

use std::path::PathBuf;

use calendar_storage::Store;
use tauri::Manager;

use commands::AppState;

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
        .setup(|app| {
            let store = open_store(app);
            if let Err(reason) = &store {
                eprintln!("[storage] could not open database: {reason}");
            }
            app.manage(AppState(store));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::list_calendars,
            commands::query_range,
            commands::create_event,
            commands::update_event,
            commands::delete_event,
            commands::set_calendar_color,
            commands::get_settings,
            commands::set_setting,
            commands::load_simulated_dataset,
            bench::bench_config,
            bench::bench_memory,
            bench::bench_report
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Universal Calendar");
}
