//! Thin Tauri integration layer for Universal Calendar.
//!
//! This crate must stay free of calendar logic: from Milestone 2 it only
//! forwards commands to the Tauri-independent `calendar-*` crates. In
//! Milestone 1 the frontend runs on simulated data and the only commands are
//! the benchmark hooks in [`bench`].

mod bench;

/// Builds and runs the desktop application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            bench::bench_config,
            bench::bench_memory,
            bench::bench_report
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Universal Calendar");
}
