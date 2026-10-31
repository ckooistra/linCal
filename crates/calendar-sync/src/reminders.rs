//! Reminder notifications: finds reminders that are due and hands them to
//! the desktop shell, without polling. The loop sleeps until the next
//! reminder (at most an hour), and wakes early when events change.
//!
//! Reminders fire for the window `(last check, now]`, so none fire twice
//! and none are skipped when the loop runs late. After the computer slept,
//! missed reminders are shown only for events that have not ended yet.

use std::sync::Arc;
use std::time::Duration;

use calendar_core::{EventTiming, MAX_REMINDER_MINUTES, Occurrence, TimeRange};
use calendar_storage::Store;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use tokio::sync::Notify;

/// The longest the loop sleeps before looking again.
const MAX_SLEEP: Duration = Duration::from_secs(3600);

/// A reminder to show now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueReminder {
    pub occurrence_id: String,
    pub title: String,
    pub location: String,
    pub start: DateTime<Utc>,
    pub all_day: bool,
    pub minutes_before: u32,
    pub fire_at: DateTime<Utc>,
}

/// Reminders firing in `(after, until]`, earliest first. Occurrences that
/// ended by `now` are left out (their reminders were missed while the
/// computer was off or asleep).
pub async fn due_reminders(
    store: &Store,
    after: DateTime<Utc>,
    until: DateTime<Utc>,
    now: DateTime<Utc>,
    display_tz: Tz,
) -> Result<Vec<DueReminder>, calendar_storage::StorageError> {
    if until <= after {
        return Ok(Vec::new());
    }
    // Occurrences starting up to the longest reminder after the window.
    let horizon = until + chrono::Duration::minutes(i64::from(MAX_REMINDER_MINUTES));
    let occurrences = store
        .query_range(TimeRange::new(after, horizon), display_tz)
        .await?;
    let mut due: Vec<DueReminder> = occurrences
        .iter()
        .filter(|o| o.timing.bounds(&display_tz).end > now)
        .flat_map(|o| reminders_of(o, display_tz))
        .filter(|r| r.fire_at > after && r.fire_at <= until)
        .collect();
    due.sort_by_key(|r| (r.fire_at, r.start));
    Ok(due)
}

fn reminders_of(o: &Occurrence, display_tz: Tz) -> impl Iterator<Item = DueReminder> + '_ {
    let start = o.timing.bounds(&display_tz).start;
    o.reminders.iter().map(move |m| DueReminder {
        occurrence_id: o.id.clone(),
        title: o.title.clone(),
        location: o.location.clone(),
        start,
        all_day: matches!(o.timing, EventTiming::AllDay { .. }),
        minutes_before: *m,
        fire_at: start - chrono::Duration::minutes(i64::from(*m)),
    })
}

/// Wakes the reminder loop after events changed.
#[derive(Clone)]
pub struct RemindersHandle {
    wake: Arc<Notify>,
}

impl RemindersHandle {
    pub fn changed(&self) {
        self.wake.notify_one();
    }
}

/// The reminder loop, to be spawned by the caller, and its handle.
/// `show` is called for each due reminder; `zone` gives the viewer's zone
/// (for all-day and floating events) each time it is needed.
pub fn reminder_loop(
    store: Store,
    zone: impl Fn() -> Tz + Send + 'static,
    show: impl Fn(DueReminder) + Send + 'static,
) -> (
    RemindersHandle,
    impl std::future::Future<Output = ()> + Send,
) {
    let wake = Arc::new(Notify::new());
    let handle = RemindersHandle {
        wake: Arc::clone(&wake),
    };
    let run = async move {
        let mut last = Utc::now();
        loop {
            let now = Utc::now();
            let tz = zone();
            match due_reminders(&store, last, now, now, tz).await {
                Ok(due) => due.into_iter().for_each(&show),
                Err(e) => eprintln!("[reminders] {e}"),
            }
            last = now;
            // Sleep until the next reminder, an hour, or a change.
            let mut wait = MAX_SLEEP;
            let window_end = now + chrono::Duration::from_std(MAX_SLEEP).expect("small");
            if let Ok(next) = due_reminders(&store, now, window_end, now, tz).await
                && let Some(first) = next.first()
            {
                let ms = (first.fire_at - now).num_milliseconds().max(0);
                wait = wait.min(Duration::from_millis(u64::try_from(ms).unwrap_or(0)));
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = wake.notified() => {}
            }
        }
    };
    (handle, run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::EventDraft;
    use chrono::TimeZone;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        BERLIN
            .with_ymd_and_hms(2026, 11, 2, h, m, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[tokio::test]
    async fn finds_due_reminders_once_and_skips_ended_events() {
        let store = Store::open_in_memory().await.unwrap();
        store.ensure_local_calendar().await.unwrap();
        let cal = store.list_calendars().await.unwrap()[0].id.clone();
        let event =
            |title: &str, start: DateTime<Utc>, rrule: Option<&str>, r: Vec<u32>| EventDraft {
                calendar_id: cal.clone(),
                title: title.into(),
                description: String::new(),
                location: String::new(),
                timing: EventTiming::Zoned {
                    start,
                    end: start + chrono::Duration::minutes(30),
                    time_zone: BERLIN,
                },
                rrule: rrule.map(Into::into),
                reminders: Some(r),
            };
        store
            .create_event("a", event("Dentist", at(10, 0), None, vec![0, 15]))
            .await
            .unwrap();
        store
            .create_event(
                "d",
                event("Stand-up", at(9, 0), Some("FREQ=DAILY"), vec![5]),
            )
            .await
            .unwrap();
        store
            .create_event("n", event("No reminder", at(9, 30), None, vec![]))
            .await
            .unwrap();

        let titles = |v: Vec<DueReminder>| {
            v.into_iter()
                .map(|r| (r.title, r.minutes_before))
                .collect::<Vec<_>>()
        };
        let due = due_reminders(&store, at(8, 0), at(10, 0), at(10, 0), BERLIN)
            .await
            .unwrap();
        // The stand-up has ended by 10:00 in this check; the dentist's two.
        assert_eq!(
            titles(due),
            vec![("Dentist".to_owned(), 15), ("Dentist".to_owned(), 0)]
        );
        let due = due_reminders(&store, at(8, 0), at(9, 0), at(9, 0), BERLIN)
            .await
            .unwrap();
        assert_eq!(titles(due), vec![("Stand-up".to_owned(), 5)]);
        // The window is half-open: (after, until].
        let due = due_reminders(&store, at(8, 55), at(9, 44), at(9, 44), BERLIN)
            .await
            .unwrap();
        assert!(due.is_empty());
        // Next day's stand-up.
        let tomorrow = at(8, 55) + chrono::Duration::days(1);
        let due = due_reminders(
            &store,
            tomorrow - chrono::Duration::minutes(1),
            tomorrow,
            tomorrow,
            BERLIN,
        )
        .await
        .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].fire_at, tomorrow);
    }

    #[tokio::test]
    async fn the_loop_shows_a_reminder_when_it_is_due() {
        let store = Store::open_in_memory().await.unwrap();
        store.ensure_local_calendar().await.unwrap();
        let cal = store.list_calendars().await.unwrap()[0].id.clone();
        let shown = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&shown);
        let (handle, run) = reminder_loop(
            store.clone(),
            || BERLIN,
            move |r| sink.lock().unwrap().push(r.title),
        );
        let task = tokio::spawn(run);
        // Due in about a second: one minute before a start 61 s away.
        let start = Utc::now() + chrono::Duration::seconds(61);
        store
            .create_event(
                "soon",
                EventDraft {
                    calendar_id: cal,
                    title: "Soon".into(),
                    description: String::new(),
                    location: String::new(),
                    timing: EventTiming::Zoned {
                        start,
                        end: start + chrono::Duration::minutes(30),
                        time_zone: BERLIN,
                    },
                    rrule: None,
                    reminders: Some(vec![1]),
                },
            )
            .await
            .unwrap();
        handle.changed();
        for _ in 0..50 {
            if !shown.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(*shown.lock().unwrap(), vec!["Soon".to_owned()]);
        task.abort();
    }
}
