//! Storage benchmark: dataset load, visible-range query and search latency against
//! an on-disk SQLite database (WAL), including recurrence expansion.
//!
//!     cargo run --release -p calendar-storage --example bench_queries

use std::time::Instant;

use calendar_core::TimeRange;
use calendar_core::sim::{self, DATASET_SIZES, DatasetOptions};
use calendar_core::timing::resolve_local;
use calendar_storage::Store;
use chrono::{Duration, NaiveDate};

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[rank]
}

fn report(name: &str, mut samples: Vec<f64>, results: usize) {
    samples.sort_by(f64::total_cmp);
    println!(
        "| {name} | {} | {:.2} | {:.2} | {:.2} | {results} |",
        samples.len(),
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
        samples.last().copied().unwrap_or_default(),
    );
}

#[tokio::main]
async fn main() {
    let anchor = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
    let dir = std::env::temp_dir().join(format!("uc-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    println!("| scenario | n | p50 ms | p95 ms | max ms | occurrences (last) |");
    println!("|---|---:|---:|---:|---:|---:|");
    for size in DATASET_SIZES {
        let path = dir.join(format!("bench-{size}.db"));
        let store = Store::open(&path).await.unwrap();
        let data = sim::generate(&DatasetOptions {
            seed: sim::DEFAULT_SEED,
            event_count: size,
            anchor,
            time_zone: TZ,
        });
        let t = Instant::now();
        store
            .replace_all(&data.accounts, &data.calendars, &data.events)
            .await
            .unwrap();
        report(
            &format!("{size}: load dataset (transaction)"),
            vec![t.elapsed().as_secs_f64() * 1e3],
            size,
        );

        for (label, days, step) in [
            ("month window (6 weeks)", 42, 30),
            ("week", 7, 7),
            ("prefetch window (3 months)", 92, 30),
        ] {
            let mut samples = Vec::new();
            let mut last = 0;
            for i in -6..6 {
                let start = anchor + Duration::days(i * step);
                let range = TimeRange::new(
                    resolve_local(&TZ, start.and_hms_opt(0, 0, 0).unwrap()),
                    resolve_local(
                        &TZ,
                        (start + Duration::days(days)).and_hms_opt(0, 0, 0).unwrap(),
                    ),
                );
                let t = Instant::now();
                last = store.query_range(range, TZ).await.unwrap().len();
                samples.push(t.elapsed().as_secs_f64() * 1e3);
            }
            report(&format!("{size}: query {label}"), samples, last);
        }

        // Search as the UI runs it: two years ahead (first 200), two back (latest 50).
        let day = |d: i64| {
            resolve_local(
                &TZ,
                (anchor + Duration::days(d)).and_hms_opt(0, 0, 0).unwrap(),
            )
        };
        for query in ["stand", "review", "zzz-no-match", "gym"] {
            let mut samples = Vec::new();
            let mut last = 0;
            for _ in 0..5 {
                let t = Instant::now();
                let (ahead, back) = tokio::join!(
                    store.search(query, TimeRange::new(day(0), day(730)), TZ, 200, false),
                    store.search(query, TimeRange::new(day(-730), day(0)), TZ, 50, true),
                );
                last = ahead.unwrap().len() + back.unwrap().len();
                samples.push(t.elapsed().as_secs_f64() * 1e3);
            }
            report(&format!("{size}: search \"{query}\""), samples, last);
        }
        store.close().await;
    }
    std::fs::remove_dir_all(&dir).ok();
}
