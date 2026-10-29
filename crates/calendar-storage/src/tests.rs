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

// --- synchronization -----------------------------------------------------

mod sync {
    use super::*;
    use crate::{IncomingObject, OpFailure, OpKind, RemoteCalendarSpec};
    use calendar_core::{Account, EventContent, ObjectContent, ProviderKind};

    const CAL_URL: &str = "https://dav.example/cal/alice/work/";

    async fn remote_store() -> (Store, String) {
        let store = Store::open_in_memory().await.unwrap();
        store
            .replace_all(&sim::accounts(), &sim::calendars(), &[])
            .await
            .unwrap();
        let account = Account {
            id: "acct-dav".into(),
            name: "alice@dav.example".into(),
            provider: ProviderKind::Caldav,
        };
        let spec = RemoteCalendarSpec {
            remote_url: CAL_URL.into(),
            name: "Work".into(),
            color: Some("#112233".into()),
            read_only: false,
            supports_sync: true,
            ctag: None,
        };
        store
            .create_remote_account(
                &account,
                &serde_json::json!({"serverUrl": "https://dav.example/"}),
                &[spec],
            )
            .await
            .unwrap();
        let cal = store.calendars_of("acct-dav").await.unwrap().remove(0);
        (store, cal.id)
    }

    fn content(uid: &str, title: &str, rrule: Option<&str>) -> ObjectContent {
        ObjectContent {
            uid: uid.into(),
            master: Some(EventContent {
                title: title.into(),
                description: String::new(),
                location: String::new(),
                timing: zoned(local(2026, 6, 1, 9, 0), 30),
                recurrence: rrule.map(|r| calendar_core::Recurrence {
                    rrule: r.into(),
                    ..Default::default()
                }),
                recurrence_id: None,
            }),
            overrides: vec![],
        }
    }

    fn incoming(href: &str, etag: &str, c: ObjectContent) -> IncomingObject {
        IncomingObject {
            href: format!("{CAL_URL}{href}"),
            etag: Some(etag.into()),
            ical: format!("ICS {etag}"),
            content: Ok(c),
        }
    }

    #[tokio::test]
    async fn local_creates_are_queued_and_settle_after_upload() {
        let (store, cal) = remote_store().await;
        let e = store
            .create_event(
                "local-1",
                draft(&cal, "New", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        assert_eq!(e.sync_status, SyncStatus::PendingSync);
        let ops = store.due_operations("acct-dav", now_ms()).await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Put);
        assert_eq!(ops[0].href, format!("{CAL_URL}local-1.ics"));
        let push = store
            .object_for_push(&cal, &ops[0].href)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((push.uid.as_str(), push.etag.as_deref()), ("local-1", None));
        assert_eq!(push.content.master.unwrap().title, "New");

        store
            .complete_put(&ops[0], Some("\"e1\""), "BEGIN:VCALENDAR...")
            .await
            .unwrap();
        assert!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .is_empty()
        );
        let shown = store.query_range(month(2026, 6), BERLIN).await.unwrap();
        assert_eq!(shown[0].sync_status, SyncStatus::Synced);
        assert_eq!(
            store.known_objects(&cal).await.unwrap(),
            vec![(ops[0].href.clone(), Some("\"e1\"".into()))]
        );
    }

    #[tokio::test]
    async fn an_edit_during_upload_is_not_lost() {
        let (store, cal) = remote_store().await;
        store
            .create_event(
                "local-1",
                draft(&cal, "v1", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        let op = store
            .due_operations("acct-dav", now_ms())
            .await
            .unwrap()
            .remove(0);
        // The user edits while the upload of v1 is in flight.
        store
            .update_event(
                "local-1",
                draft(&cal, "v2", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        store.complete_put(&op, Some("\"e1\""), "v1").await.unwrap();
        let again = store.due_operations("acct-dav", now_ms()).await.unwrap();
        assert_eq!(again.len(), 1, "v2 still needs uploading");
        assert!(again[0].generation > op.generation);
        assert_eq!(
            store.query_range(month(2026, 6), BERLIN).await.unwrap()[0].sync_status,
            SyncStatus::PendingSync
        );
        let push = store
            .object_for_push(&cal, &again[0].href)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            push.etag.as_deref(),
            Some("\"e1\""),
            "next upload is conditional on the new version"
        );
    }

    #[tokio::test]
    async fn deletes_are_queued_only_for_uploaded_objects() {
        let (store, cal) = remote_store().await;
        store
            .create_event(
                "never",
                draft(&cal, "x", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        store.delete_event("never").await.unwrap();
        assert!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .is_empty()
        );

        store
            .apply_remote_changes(
                &cal,
                &[incoming("a.ics", "\"1\"", content("a", "Remote", None))],
                &[],
                Some("tok-1"),
            )
            .await
            .unwrap();
        let id = store.query_range(month(2026, 6), BERLIN).await.unwrap()[0]
            .id
            .clone();
        assert_eq!(id, format!("{cal}/a"));
        store.delete_event(&id).await.unwrap();
        let ops = store.due_operations("acct-dav", now_ms()).await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, OpKind::Delete);
        store.complete_delete(&ops[0]).await.unwrap();
        assert!(store.known_objects(&cal).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pulled_objects_replace_rows_with_stable_ids() {
        let (store, cal) = remote_store().await;
        let mut series = content("s", "Stand-up", Some("FREQ=DAILY;COUNT=3"));
        series.overrides.push(EventContent {
            title: "Stand-up (moved)".into(),
            description: String::new(),
            location: String::new(),
            timing: zoned(local(2026, 6, 2, 15, 0), 30),
            recurrence: None,
            recurrence_id: Some(OccurrenceKey::Instant(resolve_local(
                &BERLIN,
                local(2026, 6, 2, 9, 0),
            ))),
        });
        let r = store
            .apply_remote_changes(
                &cal,
                &[incoming("s.ics", "\"1\"", series.clone())],
                &[],
                Some("t1"),
            )
            .await
            .unwrap();
        assert_eq!(r.applied, 1);
        let first = store.query_range(month(2026, 6), BERLIN).await.unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(
            first.iter().filter(|o| o.title.contains("moved")).count(),
            1
        );
        assert!(first.iter().all(|o| o.sync_status == SyncStatus::Synced));

        // A new server version replaces the rows (no duplicates, same ids).
        series.master.as_mut().unwrap().title = "Daily".into();
        store
            .apply_remote_changes(&cal, &[incoming("s.ics", "\"2\"", series)], &[], Some("t2"))
            .await
            .unwrap();
        let second = store.query_range(month(2026, 6), BERLIN).await.unwrap();
        let ids = |v: &[Occurrence]| {
            let mut i: Vec<_> = v.iter().map(|o| o.id.clone()).collect();
            i.sort();
            i
        };
        assert_eq!(ids(&first), ids(&second));
        assert_eq!(second.iter().filter(|o| o.title == "Daily").count(), 2);
        assert_eq!(
            store.remote_calendars("acct-dav").await.unwrap()[0]
                .sync_token
                .as_deref(),
            Some("t2")
        );

        store
            .apply_remote_changes(&cal, &[], &[format!("{CAL_URL}s.ics")], None)
            .await
            .unwrap();
        assert!(
            store
                .query_range(month(2026, 6), BERLIN)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn local_changes_win_until_uploaded_and_bad_objects_are_isolated() {
        let (store, cal) = remote_store().await;
        store
            .apply_remote_changes(
                &cal,
                &[incoming("a.ics", "\"1\"", content("a", "Server v1", None))],
                &[],
                None,
            )
            .await
            .unwrap();
        let id = format!("{cal}/a");
        store
            .update_event(
                &id,
                draft(&cal, "Mine", zoned(local(2026, 6, 1, 9, 0), 30), None),
            )
            .await
            .unwrap();
        let report = store
            .apply_remote_changes(
                &cal,
                &[
                    incoming("a.ics", "\"2\"", content("a", "Server v2", None)),
                    incoming(
                        "bad.ics",
                        "\"1\"",
                        content("bad", "Broken rule", Some("FREQ=NEVER")),
                    ),
                    IncomingObject {
                        href: format!("{CAL_URL}garbage.ics"),
                        etag: Some("\"1\"".into()),
                        ical: "x".into(),
                        content: Err("malformed".into()),
                    },
                    incoming("ok.ics", "\"1\"", content("ok", "Fine", None)),
                ],
                &[],
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.skipped_pending, vec![format!("{CAL_URL}a.ics")]);
        assert_eq!(report.failed.len(), 2);
        assert_eq!(report.applied, 1);
        let mut got = titles(&store, month(2026, 6)).await;
        got.sort();
        assert_eq!(got, vec!["Fine", "Mine"]);
        // Failed objects are remembered by ETag so they are not re-fetched forever.
        assert_eq!(store.known_objects(&cal).await.unwrap().len(), 4);
    }

    #[tokio::test]
    async fn conflicts_and_their_resolution() {
        let (store, cal) = remote_store().await;
        store
            .apply_remote_changes(
                &cal,
                &[incoming("a.ics", "\"1\"", content("a", "Server", None))],
                &[],
                None,
            )
            .await
            .unwrap();
        let id = format!("{cal}/a");
        store
            .update_event(
                &id,
                draft(&cal, "Mine", zoned(local(2026, 6, 1, 9, 0), 30), None),
            )
            .await
            .unwrap();
        let op = store
            .due_operations("acct-dav", now_ms())
            .await
            .unwrap()
            .remove(0);
        store
            .fail_operation(&op, "changed on server", OpFailure::Conflict)
            .await
            .unwrap();
        assert_eq!(
            store.query_range(month(2026, 6), BERLIN).await.unwrap()[0].sync_status,
            SyncStatus::Conflict
        );
        assert!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .is_empty()
        );
        let status = store
            .account_statuses()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.account_id == "acct-dav")
            .unwrap();
        assert_eq!((status.conflicts, status.pending), (1, 0));

        assert_eq!(
            store.object_of(&id).await.unwrap(),
            Some((cal.clone(), format!("{CAL_URL}a.ics")))
        );
        store
            .resolve_operation(&cal, &format!("{CAL_URL}a.ics"), true)
            .await
            .unwrap();
        let forced = store.due_operations("acct-dav", now_ms()).await.unwrap();
        assert!(forced[0].force);

        // Choosing the server version drops the local change, forgets the
        // ETag and resets the sync token so the next sync refetches it.
        store
            .apply_remote_changes(&cal, &[], &[], Some("tok"))
            .await
            .unwrap();
        store
            .resolve_operation(&cal, &format!("{CAL_URL}a.ics"), false)
            .await
            .unwrap();
        assert!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(store.known_objects(&cal).await.unwrap().is_empty());
        assert_eq!(
            store.remote_calendars("acct-dav").await.unwrap()[0].sync_token,
            None
        );
    }

    #[tokio::test]
    async fn retries_are_scheduled() {
        let (store, cal) = remote_store().await;
        store
            .create_event(
                "e",
                draft(&cal, "x", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        let op = store
            .due_operations("acct-dav", now_ms())
            .await
            .unwrap()
            .remove(0);
        let later = now_ms() + 60_000;
        store
            .fail_operation(&op, "offline", OpFailure::Retry { at_ms: later })
            .await
            .unwrap();
        assert!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.next_retry_ms("acct-dav").await.unwrap(), Some(later));
        assert_eq!(
            store.due_operations("acct-dav", later).await.unwrap()[0].attempts,
            1
        );
        // "Sync now" skips the backoff.
        store.retry_now().await.unwrap();
        assert_eq!(
            store
                .due_operations("acct-dav", now_ms())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn calendar_lists_are_reconciled() {
        let (store, cal) = remote_store().await;
        let new = RemoteCalendarSpec {
            remote_url: "https://dav.example/cal/alice/home/".into(),
            name: "Home".into(),
            color: None,
            read_only: true,
            supports_sync: false,
            ctag: None,
        };
        // Work keeps a pending change, so it survives disappearing from the server.
        store
            .create_event(
                "e",
                draft(&cal, "x", zoned(local(2026, 6, 10, 9, 0), 60), None),
            )
            .await
            .unwrap();
        assert!(
            store
                .update_remote_calendars("acct-dav", std::slice::from_ref(&new))
                .await
                .unwrap()
        );
        let names: Vec<_> = store
            .calendars_of("acct-dav")
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["Work", "Home"]);
        assert!(
            store
                .list_calendars()
                .await
                .unwrap()
                .iter()
                .any(|c| c.name == "Home" && c.read_only)
        );
    }

    #[tokio::test]
    async fn datasets_keep_real_accounts_and_moves_are_refused() {
        let (store, cal) = remote_store().await;
        store
            .apply_remote_changes(
                &cal,
                &[incoming("a.ics", "\"1\"", content("a", "Keep me", None))],
                &[],
                None,
            )
            .await
            .unwrap();
        let data = sim::generate(&DatasetOptions {
            seed: 1,
            event_count: 100,
            anchor: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
            time_zone: BERLIN,
        });
        store
            .replace_all(&data.accounts, &data.calendars, &data.events)
            .await
            .unwrap();
        assert!(
            titles(&store, month(2026, 6))
                .await
                .contains(&"Keep me".to_owned())
        );
        assert_eq!(store.remote_accounts().await.unwrap().len(), 1);
        assert!(matches!(
            store
                .update_event(
                    &format!("{cal}/a"),
                    draft(
                        "cal-work",
                        "Moved",
                        zoned(local(2026, 6, 1, 9, 0), 30),
                        None
                    )
                )
                .await,
            Err(StorageError::Unsupported(_))
        ));
        store.remove_account("acct-dav").await.unwrap();
        assert!(
            !titles(&store, month(2026, 6))
                .await
                .contains(&"Keep me".to_owned())
        );
    }
}
