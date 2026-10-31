//! Two clients syncing through a real CalDAV server. Skipped unless
//! configured (see crates/calendar-caldav/tests/real_server.rs), e.g.:
//!
//!     scripts/dev-radicale.sh start
//!     UC_CALDAV_URL=http://127.0.0.1:5232/ UC_CALDAV_USER=test UC_CALDAV_PASSWORD=test \
//!       cargo test -p calendar-sync --test real_server -- --nocapture
//!
//! Only touches events it creates in the calendar named UC_CALDAV_CALENDAR
//! (default "UC Test") and deletes them at the end.

use std::sync::Arc;

use calendar_core::timing::resolve_local;
use calendar_core::{EventDraft, EventTiming, SyncStatus, TimeRange};
use calendar_storage::Store;
use calendar_sync::{MemoryCredentials, SyncEngine, default_factory};
use chrono::{Duration, NaiveDate};

const TZ: chrono_tz::Tz = chrono_tz::Europe::Berlin;

struct Client {
    store: Store,
    engine: SyncEngine,
    account: String,
    calendar: String,
}

async fn client(url: &str, user: &str, password: &str, calendar_name: &str) -> Client {
    let store = Store::open_in_memory().await.unwrap();
    let engine = SyncEngine::new(
        store.clone(),
        Arc::new(MemoryCredentials::default()),
        default_factory(),
        Arc::new(|_| {}),
    );
    let account = engine
        .add_caldav_account(url, user, password)
        .await
        .expect("add account");
    engine.sync_account(&account).await.expect("initial sync");
    let calendar = store
        .calendars_of(&account)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name == calendar_name)
        .expect("test calendar")
        .id;
    Client {
        store,
        engine,
        account,
        calendar,
    }
}

impl Client {
    async fn sync(&self) {
        self.engine.sync_account(&self.account).await.expect("sync");
    }
    async fn find(&self, title_prefix: &str) -> Vec<calendar_core::Occurrence> {
        let range = TimeRange::new(
            resolve_local(
                &TZ,
                NaiveDate::from_ymd_opt(2026, 11, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
            resolve_local(
                &TZ,
                NaiveDate::from_ymd_opt(2026, 12, 1)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
            ),
        );
        self.store
            .query_range(range, TZ)
            .await
            .unwrap()
            .into_iter()
            .filter(|o| o.calendar_id == self.calendar && o.title.starts_with(title_prefix))
            .collect()
    }
}

fn timing(day: u32, hour: u32) -> EventTiming {
    let start = resolve_local(
        &TZ,
        NaiveDate::from_ymd_opt(2026, 11, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap(),
    );
    EventTiming::Zoned {
        start,
        end: start + Duration::hours(1),
        time_zone: TZ,
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

#[tokio::test]
async fn two_clients_stay_in_sync() {
    let (Ok(url), Ok(user), Ok(password)) = (
        std::env::var("UC_CALDAV_URL"),
        std::env::var("UC_CALDAV_USER"),
        std::env::var("UC_CALDAV_PASSWORD"),
    ) else {
        eprintln!("UC_CALDAV_URL not set; skipping");
        return;
    };
    let cal_name = std::env::var("UC_CALDAV_CALENDAR").unwrap_or_else(|_| "UC Test".into());
    let tag = format!("UC-{}", std::process::id());
    let a = client(&url, &user, &password, &cal_name).await;
    let b = client(&url, &user, &password, &cal_name).await;

    // Create in A → visible in B.
    let id = format!("{tag}-one");
    a.store
        .create_event(
            &id,
            draft(&a.calendar, &format!("{tag} meeting"), timing(10, 9), None),
        )
        .await
        .unwrap();
    a.sync().await;
    assert_eq!(a.find(&tag).await[0].sync_status, SyncStatus::Synced);
    b.sync().await;
    let in_b = b.find(&tag).await;
    assert_eq!(in_b.len(), 1, "B sees A's event");

    // Edit in B → visible in A.
    b.store
        .update_event(
            &in_b[0].id,
            draft(
                &b.calendar,
                &format!("{tag} meeting (moved)"),
                timing(11, 15),
                None,
            ),
        )
        .await
        .unwrap();
    b.sync().await;
    a.sync().await;
    let in_a = a.find(&tag).await;
    assert_eq!(in_a[0].title, format!("{tag} meeting (moved)"));
    assert_eq!(in_a[0].timing, timing(11, 15));

    // Recurring series in A; B moves one occurrence; A sees the override.
    let series = format!("{tag}-series");
    a.store
        .create_event(
            &series,
            draft(
                &a.calendar,
                &format!("{tag} weekly"),
                timing(2, 8),
                Some("FREQ=WEEKLY;COUNT=4"),
            ),
        )
        .await
        .unwrap();
    a.sync().await;
    b.sync().await;
    let weekly_b: Vec<_> = b.find(&format!("{tag} weekly")).await;
    assert_eq!(weekly_b.len(), 4);
    b.store
        .update_occurrence(
            &weekly_b[1].id,
            draft(
                &b.calendar,
                &format!("{tag} weekly (special)"),
                timing(10, 17),
                None,
            ),
            TZ,
        )
        .await
        .unwrap();
    b.sync().await;
    a.sync().await;
    let weekly_a = a.find(&format!("{tag} weekly")).await;
    assert_eq!(weekly_a.len(), 4, "moved, not duplicated");
    assert!(
        weekly_a
            .iter()
            .any(|o| o.title.ends_with("(special)") && o.timing == timing(10, 17))
    );

    // Concurrent edits: B saves first, A gets a conflict instead of overwriting.
    let one_a = a.find(&format!("{tag} meeting")).await.remove(0);
    let one_b = b.find(&format!("{tag} meeting")).await.remove(0);
    b.store
        .update_event(
            &one_b.id,
            draft(
                &b.calendar,
                &format!("{tag} meeting by B"),
                timing(11, 15),
                None,
            ),
        )
        .await
        .unwrap();
    b.sync().await;
    a.store
        .update_event(
            &one_a.id,
            draft(
                &a.calendar,
                &format!("{tag} meeting by A"),
                timing(11, 15),
                None,
            ),
        )
        .await
        .unwrap();
    a.sync().await;
    let conflicted = a.find(&format!("{tag} meeting")).await.remove(0);
    assert_eq!(conflicted.sync_status, SyncStatus::Conflict);
    let (cal, href) = a.store.object_of(&conflicted.id).await.unwrap().unwrap();
    a.store
        .resolve_operation(&cal, &href, calendar_storage::Resolution::UseServer)
        .await
        .unwrap();
    a.sync().await;
    assert_eq!(
        a.find(&format!("{tag} meeting")).await[0].title,
        format!("{tag} meeting by B")
    );

    // "This and following" in A from the third occurrence: two series on
    // the server; B sees the earlier part unchanged and the later one moved.
    let tail = format!("{tag}-tail");
    a.store
        .update_following(
            &weekly_a[2].id,
            &tail,
            draft(
                &a.calendar,
                &format!("{tag} weekly later"),
                timing(16, 10),
                Some("FREQ=WEEKLY;COUNT=2"),
            ),
        )
        .await
        .unwrap();
    a.sync().await;
    b.sync().await;
    let split_b: Vec<(String, EventTiming)> = b
        .find(&format!("{tag} weekly"))
        .await
        .into_iter()
        .map(|o| (o.title, o.timing))
        .collect();
    assert_eq!(
        split_b,
        vec![
            (format!("{tag} weekly"), timing(2, 8)),
            (format!("{tag} weekly (special)"), timing(10, 17)),
            (format!("{tag} weekly later"), timing(16, 10)),
            (format!("{tag} weekly later"), timing(23, 10)),
        ]
    );
    let weekly_a = a.find(&format!("{tag} weekly")).await;

    // Clean up from A; B sees the deletions.
    let one = a.find(&format!("{tag} meeting")).await.remove(0);
    a.store.delete_event(&one.id).await.unwrap();
    a.store.delete_series(&weekly_a[0].id).await.unwrap();
    a.store.delete_series(&weekly_a[3].id).await.unwrap();
    a.sync().await;
    b.sync().await;
    assert!(b.find(&tag).await.is_empty(), "deletions propagate");
}
