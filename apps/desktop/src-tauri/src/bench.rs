//! Benchmark hooks used by `scripts/bench.sh`.
//!
//! Benchmark mode is enabled only through environment variables set by the
//! person launching the app, never by the frontend:
//!
//! - `UC_BENCH=1` enables the scripted benchmark in the UI;
//! - `UC_BENCH_SIZES=100,1000,5000` selects dataset sizes;
//! - `UC_BENCH_OUT=/path/report.json` is where the report is written
//!   (stdout when unset). The app exits after writing the report.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

const DEFAULT_SIZES: [u32; 3] = [100, 1000, 5000];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchConfig {
    enabled: bool,
    sizes: Vec<u32>,
}

/// Resident memory of the app and the WebKit helper processes it spawned.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemorySample {
    app_kb: u64,
    children_kb: u64,
    total_kb: u64,
    processes: usize,
}

fn bench_enabled() -> bool {
    std::env::var("UC_BENCH").is_ok_and(|v| v == "1")
}

/// Reports whether benchmark mode was requested and with which dataset sizes.
#[tauri::command]
pub fn bench_config() -> BenchConfig {
    let sizes = std::env::var("UC_BENCH_SIZES")
        .ok()
        .map(|v| parse_sizes(&v))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SIZES.to_vec());
    BenchConfig {
        enabled: bench_enabled(),
        sizes,
    }
}

/// Samples resident memory; `None` on platforms without `/proc`.
#[tauri::command]
pub fn bench_memory() -> Option<MemorySample> {
    sample_memory()
}

/// Writes the benchmark report (with a final memory sample) and exits.
///
/// # Errors
/// Fails when benchmark mode is off or the report cannot be written.
#[tauri::command]
pub async fn bench_report(app: tauri::AppHandle, report: serde_json::Value) -> Result<(), String> {
    if !bench_enabled() {
        return Err("benchmark mode is not enabled".into());
    }
    let output = serde_json::json!({
        "report": report,
        "finalMemory": sample_memory(),
    });
    let text = serde_json::to_string_pretty(&output).map_err(|e| e.to_string())?;
    match std::env::var_os("UC_BENCH_OUT").map(PathBuf::from) {
        Some(path) => tauri::async_runtime::spawn_blocking(move || std::fs::write(path, text))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("could not write benchmark report: {e}"))?,
        None => println!("{text}"),
    }
    app.exit(0);
    Ok(())
}

fn parse_sizes(value: &str) -> Vec<u32> {
    value
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .filter(|&n| n > 0)
        .collect()
}

/// `VmRSS` in KiB from the contents of `/proc/<pid>/status`.
fn parse_vm_rss(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
}

/// Parent pid from the contents of `/proc/<pid>/stat`. The command name may
/// contain spaces and parentheses, so parse after the last `)`.
fn parse_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// All transitive children of `root`, given a pid → parent pid map.
fn descendants(parents: &HashMap<u32, u32>, root: u32) -> Vec<u32> {
    let mut result = Vec::new();
    let mut frontier = vec![root];
    while let Some(pid) = frontier.pop() {
        for (&child, &parent) in parents {
            if parent == pid {
                result.push(child);
                frontier.push(child);
            }
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn sample_memory() -> Option<MemorySample> {
    let rss = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| parse_vm_rss(&s))
    };
    let mut parents = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(ppid) = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|s| parse_ppid(&s))
        {
            parents.insert(pid, ppid);
        }
    }
    let me = std::process::id();
    let app_kb = rss(me)?;
    let children = descendants(&parents, me);
    let children_kb = children.iter().filter_map(|&pid| rss(pid)).sum();
    Some(MemorySample {
        app_kb,
        children_kb,
        total_kb: app_kb + children_kb,
        processes: children.len() + 1,
    })
}

#[cfg(not(target_os = "linux"))]
fn sample_memory() -> Option<MemorySample> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_from_status() {
        let status = "Name:\tapp\nVmPeak:\t  999 kB\nVmRSS:\t  123456 kB\nThreads:\t9\n";
        assert_eq!(parse_vm_rss(status), Some(123_456));
        assert_eq!(parse_vm_rss("Name:\tkernel-thread\n"), None);
    }

    #[test]
    fn parses_ppid_even_with_odd_process_names() {
        assert_eq!(parse_ppid("42 (WebKit) Web) S 7 42 42 0"), Some(7));
        assert_eq!(parse_ppid("garbage"), None);
    }

    #[test]
    fn finds_transitive_children() {
        let parents = HashMap::from([(2, 1), (3, 2), (4, 3), (5, 99)]);
        let mut found = descendants(&parents, 1);
        found.sort_unstable();
        assert_eq!(found, vec![2, 3, 4]);
    }

    #[test]
    fn parses_size_lists() {
        assert_eq!(parse_sizes("100, 1000,x,0,5000"), vec![100, 1000, 5000]);
    }
}
