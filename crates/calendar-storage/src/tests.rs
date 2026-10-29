use super::*;
use calendar_core::EventTiming;
use calendar_core::sim::{self, DatasetOptions};
use calendar_core::timing::resolve_local;
use chrono::{Duration, NaiveDate, NaiveDateTime};

const BERLIN: Tz = chrono_tz::Europe::Berlin;

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap()
}

fn zoned(start: NaiveDateTime, minutes: i64) -> EventTiming {
    let s = resolve_local(&BERLIN, start);
    EventTiming::Zoned {
        start: s,
        end: s + Duration::minutes(minutes),
        time_zone: BERLIN,
    }
}

fn draft(calendar: &str, title: &str, timing: EventTiming, rrule: Option<&str>) -> EventDraft {
    EventDraft {
        calendar_id: calendar.into(),
        title: title.into(),
        description: String::new(),
        location: String::new(),
        timing,
        rrule: rrule.map(Into::into),
    }
}

fn month(y: i32, m: u32) -> TimeRange {
    let start = NaiveDate::from_ymd_opt(y, m, 1).unwrap();
    let end = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    }
    .unwrap();
    TimeRange::new(
        resolve_local(&BERLIN, start.and_hms_opt(0, 0, 0).unwrap()),
        resolve_local(&BERLIN, end.and_hms_opt(0, 0, 0).unwrap()),
    )
}

async fn store_with_calendars() -> Store {
    let store = Store::open_in_memory().await.unwrap();
    store
        .replace_all(&sim::accounts(), &sim::calendars(), &[])
        .await
        .unwrap();
    store
}

async fn titles(store: &Store, range: TimeRange) -> Vec<String> {
    store
        .query_range(range, BERLIN)
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect()
}

#[tokio::test]
async fn crud_round_trip() {
    let store = store_with_calendars().await;
    let created = store
        .create_event(
            "e1",
            draft(
                "cal-work",
                "Review",
                zoned(local(2026, 6, 10, 9, 0), 60),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(store.get_event("e1").await.unwrap().unwrap(), created);
    assert_eq!(titles(&store, month(2026, 6)).await, vec!["Review"]);
    assert!(titles(&store, month(2026, 7)).await.is_empty());

    let updated = store
        .update_event(
            "e1",
            draft(
                "cal-team",
                "Review 2",
                zoned(local(2026, 7, 1, 9, 0), 30),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, 2);
    assert_eq!(titles(&store, month(2026, 7)).await, vec!["Review 2"]);

    store.delete_event("e1").await.unwrap();
    assert!(store.get_event("e1").await.unwrap().is_none());
}

#[tokio::test]
async fn rejects_duplicates_read_only_and_invalid_input() {
    let store = store_with_calendars().await;
    let d = draft("cal-work", "A", zoned(local(2026, 6, 10, 9, 0), 60), None);
    store.create_event("dup", d.clone()).await.unwrap();
    assert!(matches!(
        store.create_event("dup", d.clone()).await,
        Err(StorageError::AlreadyExists(_))
    ));
    let ro = draft(
        "cal-holidays",
        "B",
        zoned(local(2026, 6, 10, 9, 0), 60),
        None,
    );
    assert!(matches!(
        store.create_event("ro", ro).await,
        Err(StorageError::ReadOnly(_))
    ));
    let backwards = draft("cal-work", "C", zoned(local(2026, 6, 10, 9, 0), -10), None);
    assert!(matches!(
        store.create_event("bad", backwards).await,
        Err(StorageError::Invalid(_))
    ));
    let bad_rule = draft(
        "cal-work",
        "D",
        zoned(local(2026, 6, 10, 9, 0), 10),
        Some("FREQ=NEVER"),
    );
    assert!(matches!(
        store.create_event("bad2", bad_rule).await,
        Err(StorageError::Invalid(_))
    ));
    let unknown = draft("cal-nope", "E", zoned(local(2026, 6, 10, 9, 0), 10), None);
    assert!(matches!(
        store.create_event("x", unknown).await,
        Err(StorageError::UnknownCalendar(_))
    ));
    assert_eq!(store.event_count().await.unwrap(), 1);
}

#[tokio::test]
async fn persists_across_reopen() {
    let dir = std::env::temp_dir().join(format!("uc-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("calendar.db");
    {
        let store = Store::open(&path).await.unwrap();
        store
            .replace_all(&sim::accounts(), &sim::calendars(), &[])
            .await
            .unwrap();
        store
            .create_event(
                "keep",
                draft(
                    "cal-work",
                    "Persisted",
                    zoned(local(2026, 6, 10, 9, 0), 60),
                    None,
                ),
            )
            .await
            .unwrap();
        store
            .set_setting("theme", &serde_json::json!("dark"))
            .await
            .unwrap();
        store.close().await;
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(titles(&store, month(2026, 6)).await, vec!["Persisted"]);
    assert_eq!(store.settings().await.unwrap()["theme"], "dark");
    store.close().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn recurring_series_expand_per_range() {
    let store = store_with_calendars().await;
    store
        .create_event(
            "s",
            draft(
                "cal-team",
                "Stand-up",
                zoned(local(2026, 1, 5, 9, 30), 15),
                Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"),
            ),
        )
        .await
        .unwrap();
    let june = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    assert_eq!(june.len(), 22);
    assert!(june.iter().all(|o| o.series_id.as_deref() == Some("s")));
    // Unbounded: still present years later, and still at 09:30 local.
    let later = store.query_range(month(2030, 3), BERLIN).await.unwrap();
    assert!(!later.is_empty());
    let EventTiming::Zoned { start, .. } = later[0].timing else {
        panic!()
    };
    assert_eq!(
        start.with_timezone(&BERLIN).format("%H:%M").to_string(),
        "09:30"
    );
}

#[tokio::test]
async fn single_occurrence_edits_and_deletes() {
    let store = store_with_calendars().await;
    store
        .create_event(
            "s",
            draft(
                "cal-team",
                "Daily",
                zoned(local(2026, 6, 1, 9, 0), 30),
                Some("FREQ=DAILY;COUNT=5"),
            ),
        )
        .await
        .unwrap();
    let occ = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    assert_eq!(occ.len(), 5);

    // Move the second occurrence to the afternoon.
    let second = occ[1].id.clone();
    let moved = store
        .update_occurrence(
            &second,
            draft(
                "cal-team",
                "Daily (moved)",
                zoned(local(2026, 6, 2, 15, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    assert_eq!(moved.id, second);
    let after = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    assert_eq!(after.len(), 5);
    assert_eq!(
        after.iter().filter(|o| o.title == "Daily (moved)").count(),
        1
    );
    // Editing it again updates the same override.
    store
        .update_occurrence(
            &second,
            draft(
                "cal-team",
                "Daily (moved twice)",
                zoned(local(2026, 6, 2, 16, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    assert_eq!(store.event_count().await.unwrap(), 2);

    // Cancel the third occurrence.
    store.delete_occurrence(&occ[2].id).await.unwrap();
    assert_eq!(
        store
            .query_range(month(2026, 6), BERLIN)
            .await
            .unwrap()
            .len(),
        4
    );

    // An occurrence that the rule never generates cannot be overridden.
    let bogus = occurrence_id(
        "s",
        &OccurrenceKey::Instant(resolve_local(&BERLIN, local(2026, 6, 20, 9, 0))),
    );
    assert!(matches!(
        store
            .update_occurrence(
                &bogus,
                draft("cal-team", "x", zoned(local(2026, 6, 20, 9, 0), 30), None),
                BERLIN
            )
            .await,
        Err(StorageError::NotFound(_))
    ));
    // Overrides cannot change calendar.
    assert!(matches!(
        store
            .update_occurrence(
                &second,
                draft("cal-work", "x", zoned(local(2026, 6, 2, 9, 0), 30), None),
                BERLIN
            )
            .await,
        Err(StorageError::Unsupported(_))
    ));

    // Deleting the series removes its overrides too.
    store.delete_event("s").await.unwrap();
    assert_eq!(store.event_count().await.unwrap(), 0);
}

#[tokio::test]
async fn series_edits_shift_start_exceptions_and_overrides() {
    let store = store_with_calendars().await;
    store
        .create_event(
            "s",
            draft(
                "cal-team",
                "Daily",
                zoned(local(2026, 6, 1, 9, 0), 30),
                Some("FREQ=DAILY;COUNT=5"),
            ),
        )
        .await
        .unwrap();
    let occ = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    store
        .update_occurrence(
            &occ[1].id,
            draft(
                "cal-team",
                "Moved",
                zoned(local(2026, 6, 2, 15, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    store.delete_occurrence(&occ[3].id).await.unwrap();

    // Edit the whole series from the 3rd occurrence: one hour later, new title.
    let series = store
        .update_series(
            &occ[2].id,
            draft(
                "cal-work",
                "Daily v2",
                zoned(local(2026, 6, 3, 10, 0), 45),
                Some("FREQ=DAILY;COUNT=5"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(series.timing, zoned(local(2026, 6, 1, 10, 0), 45));
    let after = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    // 5 slots - 1 cancelled; the override survives with its own time.
    assert_eq!(after.len(), 4);
    assert_eq!(after.iter().filter(|o| o.title == "Moved").count(), 1);
    assert!(after.iter().all(|o| o.calendar_id == "cal-work"));
    assert_eq!(after.iter().filter(|o| o.title == "Daily v2").count(), 3);

    // "Does not repeat" turns the series into the edited occurrence.
    let single = store
        .update_series(
            &after[0].id,
            draft(
                "cal-work",
                "Once",
                zoned(local(2026, 6, 20, 8, 0), 30),
                None,
            ),
        )
        .await
        .unwrap();
    assert!(single.recurrence.is_none());
    assert_eq!(titles(&store, month(2026, 6)).await, vec!["Once"]);
    assert_eq!(store.event_count().await.unwrap(), 1);
}

#[tokio::test]
async fn all_day_events_respect_the_viewer_zone() {
    let store = store_with_calendars().await;
    let day = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();
    store
        .create_event(
            "a",
            draft(
                "cal-personal",
                "Day off",
                EventTiming::AllDay {
                    start_date: day,
                    end_date: day + Duration::days(1),
                },
                None,
            ),
        )
        .await
        .unwrap();
    // Visible at the end of June for Berlin and Tokyo, never in July.
    for tz in [
        BERLIN,
        chrono_tz::Asia::Tokyo,
        chrono_tz::America::Los_Angeles,
    ] {
        let june = TimeRange::new(
            resolve_local(&tz, local(2026, 6, 1, 0, 0)),
            resolve_local(&tz, local(2026, 7, 1, 0, 0)),
        );
        let july = TimeRange::new(
            resolve_local(&tz, local(2026, 7, 1, 0, 0)),
            resolve_local(&tz, local(2026, 8, 1, 0, 0)),
        );
        assert_eq!(store.query_range(june, tz).await.unwrap().len(), 1, "{tz}");
        assert!(
            store.query_range(july, tz).await.unwrap().is_empty(),
            "{tz}"
        );
    }
}

#[tokio::test]
async fn simulated_dataset_loads_and_queries() {
    let store = Store::open_in_memory().await.unwrap();
    let data = sim::generate(&DatasetOptions {
        seed: 7,
        event_count: 5000,
        anchor: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
        time_zone: BERLIN,
    });
    store
        .replace_all(&data.accounts, &data.calendars, &data.events)
        .await
        .unwrap();
    assert_eq!(store.event_count().await.unwrap(), 5000);
    let june = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    assert!(june.len() > 400, "{}", june.len());
    // The moved 1:1 replaces its slot rather than duplicating it.
    let moved: Vec<_> = june
        .iter()
        .filter(|o| o.title.ends_with("(moved)"))
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(
        moved[0].series_id.as_deref(),
        Some(moved[0].id.split("::").next().unwrap())
    );
    let slot = moved[0].recurrence_id.unwrap();
    assert!(
        june.iter()
            .filter(|o| o.recurrence_id == Some(slot))
            .count()
            == 1
    );
}

#[tokio::test]
async fn calendar_colours_and_settings() {
    let store = store_with_calendars().await;
    store
        .set_calendar_color("cal-work", "#123456")
        .await
        .unwrap();
    assert!(
        store
            .list_calendars()
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == "cal-work" && c.color == "#123456")
    );
    assert!(store.set_calendar_color("cal-work", "red").await.is_err());
    assert!(matches!(
        store.set_calendar_color("nope", "#000000").await,
        Err(StorageError::UnknownCalendar(_))
    ));
    store
        .set_setting("hidden", &serde_json::json!(["cal-team"]))
        .await
        .unwrap();
    store
        .set_setting("hidden", &serde_json::json!([]))
        .await
        .unwrap();
    assert_eq!(
        store.settings().await.unwrap()["hidden"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn editing_a_series_through_a_moved_occurrence_keeps_the_series_time() {
    let store = store_with_calendars().await;
    store
        .create_event(
            "s",
            draft(
                "cal-team",
                "Daily",
                zoned(local(2026, 6, 1, 9, 0), 30),
                Some("FREQ=DAILY;COUNT=3"),
            ),
        )
        .await
        .unwrap();
    let occ = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    store
        .update_occurrence(
            &occ[1].id,
            draft(
                "cal-team",
                "Daily",
                zoned(local(2026, 6, 2, 15, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    // Rename the series from the moved occurrence without changing its time.
    let series = store
        .update_series(
            &occ[1].id,
            draft(
                "cal-team",
                "Renamed",
                zoned(local(2026, 6, 2, 15, 0), 30),
                Some("FREQ=DAILY;COUNT=3"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(series.timing, zoned(local(2026, 6, 1, 9, 0), 30));
    let after = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    assert!(after.iter().all(|o| o.title == "Renamed"));
    assert_eq!(after.len(), 3);
}

#[tokio::test]
async fn series_text_changes_reach_overrides_that_kept_the_series_value() {
    let store = store_with_calendars().await;
    store
        .create_event(
            "s",
            draft(
                "cal-team",
                "Sync",
                zoned(local(2026, 6, 1, 9, 0), 30),
                Some("FREQ=DAILY;COUNT=3"),
            ),
        )
        .await
        .unwrap();
    let occ = store.query_range(month(2026, 6), BERLIN).await.unwrap();
    // One override only moved, one also got its own title.
    store
        .update_occurrence(
            &occ[1].id,
            draft(
                "cal-team",
                "Sync",
                zoned(local(2026, 6, 2, 15, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    store
        .update_occurrence(
            &occ[2].id,
            draft(
                "cal-team",
                "Special",
                zoned(local(2026, 6, 3, 15, 0), 30),
                None,
            ),
            BERLIN,
        )
        .await
        .unwrap();
    store
        .update_series(
            &occ[0].id,
            draft(
                "cal-team",
                "Daily sync",
                zoned(local(2026, 6, 1, 9, 0), 30),
                Some("FREQ=DAILY;COUNT=3"),
            ),
        )
        .await
        .unwrap();
    let mut got = titles(&store, month(2026, 6)).await;
    got.sort();
    assert_eq!(got, vec!["Daily sync", "Daily sync", "Special"]);
}
