//! Deterministic simulated dataset for development and benchmarks.
//!
//! The same options always produce the same rows. The dataset stresses a
//! calendar UI: busy days with many overlapping events, all-day and
//! multi-day events, read-only calendars, and real recurring series
//! (RRULE masters with EXDATEs and moved occurrences).

use chrono::{Datelike, Duration, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::model::{
    Account, Calendar, CalendarEvent, EventTiming, ProviderKind, Recurrence, SeriesLink,
    SyncStatus, occurrence_id,
};
use crate::occurrence_key::OccurrenceKey;
use crate::timing::resolve_local;

pub const DEFAULT_SEED: u32 = 20_261_029;
pub const DATASET_SIZES: [usize; 3] = [100, 1000, 5000];

const WINDOW_DAYS: i64 = 183;
const BUSY_DAY_SPREAD: i64 = 30;
const EVENTS_PER_BUSY_DAY: usize = 12;

pub struct DatasetOptions {
    pub seed: u32,
    /// Number of stored rows (series masters and overrides count as one each).
    pub event_count: usize,
    pub anchor: NaiveDate,
    pub time_zone: Tz,
}

pub struct Dataset {
    pub accounts: Vec<Account>,
    pub calendars: Vec<Calendar>,
    pub events: Vec<CalendarEvent>,
}

/// mulberry32, matching the frontend test fixture generator.
struct Prng(u32);

impl Prng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    fn int(&mut self, min: i64, max: i64) -> i64 {
        min + (self.next() * (max - min + 1) as f64) as i64
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() * items.len() as f64) as usize]
    }
}

fn account(id: &str, name: &str) -> Account {
    Account {
        id: id.into(),
        name: name.into(),
        provider: ProviderKind::Simulated,
    }
}

fn calendar(id: &str, account_id: &str, name: &str, color: &str, read_only: bool) -> Calendar {
    Calendar {
        id: id.into(),
        account_id: account_id.into(),
        name: name.into(),
        color: color.into(),
        read_only,
    }
}

pub fn accounts() -> Vec<Account> {
    vec![
        account("acct-personal", "Personal (simulated CalDAV)"),
        account("acct-work", "Work (simulated Google)"),
        account("acct-shared", "Shared (simulated ICS)"),
    ]
}

pub fn calendars() -> Vec<Calendar> {
    vec![
        calendar(
            "cal-personal",
            "acct-personal",
            "Personal",
            "#3b82f6",
            false,
        ),
        calendar("cal-family", "acct-personal", "Family", "#10b981", false),
        calendar("cal-fitness", "acct-personal", "Fitness", "#f59e0b", false),
        calendar("cal-work", "acct-work", "Work", "#8b5cf6", false),
        calendar("cal-team", "acct-work", "Team", "#ec4899", false),
        calendar("cal-projects", "acct-work", "Projects", "#06b6d4", false),
        calendar("cal-holidays", "acct-shared", "Holidays", "#ef4444", true),
        calendar("cal-birthdays", "acct-shared", "Birthdays", "#84cc16", true),
    ]
}

const TIMED_TOPICS: [(&str, &[&str]); 6] = [
    (
        "cal-personal",
        &[
            "Dentist",
            "Haircut",
            "Coffee with Sam",
            "Call the bank",
            "Book club",
            "Car service",
        ],
    ),
    (
        "cal-family",
        &[
            "School pickup",
            "Parents' evening",
            "Grocery run",
            "Piano lesson",
            "Movie night",
        ],
    ),
    (
        "cal-fitness",
        &["Run", "Yoga", "Swimming", "Climbing", "Cycling"],
    ),
    (
        "cal-work",
        &[
            "Design review",
            "Budget sync",
            "Customer call",
            "Hiring panel",
            "Quarterly planning",
        ],
    ),
    (
        "cal-team",
        &[
            "Sprint planning",
            "Retro",
            "Pairing session",
            "Demo",
            "Architecture chat",
        ],
    ),
    (
        "cal-projects",
        &[
            "Milestone check",
            "Release prep",
            "Bug triage",
            "Roadmap",
            "Spec writing",
        ],
    ),
];
const HOLIDAYS: [&str; 4] = ["Public holiday", "Bank holiday", "National day", "Festival"];
const NAMES: [&str; 8] = [
    "Alex", "Jordan", "Robin", "Morgan", "Casey", "Taylor", "Jamie", "Riley",
];
const LOCATIONS: [&str; 8] = [
    "",
    "",
    "Room 4.12",
    "Online",
    "Café Central",
    "Gym",
    "Home",
    "Office",
];
const DURATIONS_MIN: [i64; 10] = [15, 30, 30, 45, 60, 60, 60, 90, 120, 180];
const START_HOURS: [u32; 19] = [
    7, 8, 9, 9, 10, 10, 11, 11, 12, 13, 14, 14, 15, 15, 16, 17, 18, 19, 20,
];

struct Builder {
    seed: u32,
    tz: Tz,
    rng: Prng,
    events: Vec<CalendarEvent>,
    seq: usize,
}

impl Builder {
    fn next_id(&mut self) -> String {
        let id = format!("sim-{}-{}", self.seed, self.seq);
        self.seq += 1;
        id
    }

    fn zoned(&self, day: NaiveDate, hour: u32, minute: u32, minutes: i64) -> EventTiming {
        let start = resolve_local(&self.tz, day.and_hms_opt(hour, minute, 0).expect("valid"));
        EventTiming::Zoned {
            start,
            end: start + Duration::minutes(minutes),
            time_zone: self.tz,
        }
    }

    fn push(
        &mut self,
        id: String,
        calendar_id: &str,
        title: &str,
        timing: EventTiming,
        recurrence: Option<Recurrence>,
        series: Option<SeriesLink>,
    ) {
        let location = self.rng.pick(&LOCATIONS).to_string();
        let epoch = chrono::DateTime::<Utc>::UNIX_EPOCH;
        self.events.push(CalendarEvent {
            reminders: Vec::new(),
            id,
            calendar_id: calendar_id.into(),
            title: title.into(),
            description: String::new(),
            location,
            timing,
            recurrence,
            series,
            sync_status: SyncStatus::Synced,
            revision: 1,
            created_at: epoch,
            updated_at: epoch,
        });
    }
}

fn weekday_on_or_after(day: NaiveDate, weekday: chrono::Weekday) -> NaiveDate {
    let mut d = day;
    while d.weekday() != weekday {
        d += Duration::days(1);
    }
    d
}

pub fn generate(options: &DatasetOptions) -> Dataset {
    let mut b = Builder {
        seed: options.seed,
        tz: options.time_zone,
        rng: Prng(options.seed),
        events: Vec::with_capacity(options.event_count),
        seq: 0,
    };
    let n = options.event_count;
    let anchor = options.anchor;
    let window_start = anchor - Duration::days(WINDOW_DAYS);

    // Recurring series (only when the dataset is large enough to afford them).
    if n >= 20 {
        // Weekday stand-up, unbounded, with two cancelled occurrences.
        let standup_start = weekday_on_or_after(window_start, chrono::Weekday::Mon);
        let standup_timing = b.zoned(standup_start, 9, 30, 15);
        let cancelled = |b: &Builder, offset: i64| {
            let day = weekday_on_or_after(anchor + Duration::days(offset), chrono::Weekday::Wed);
            let EventTiming::Zoned { start, .. } = b.zoned(day, 9, 30, 15) else {
                unreachable!()
            };
            OccurrenceKey::Instant(start)
        };
        let exdates = vec![cancelled(&b, 7), cancelled(&b, 21)];
        let id = b.next_id();
        b.push(
            id,
            "cal-team",
            "Team stand-up",
            standup_timing,
            Some(Recurrence {
                rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into(),
                rdates: Vec::new(),
                exdates,
            }),
            None,
        );

        // Gym three times a week for a year.
        let gym = b.zoned(
            weekday_on_or_after(window_start, chrono::Weekday::Mon),
            18,
            0,
            60,
        );
        let until = (anchor + Duration::days(WINDOW_DAYS)).format("%Y%m%dT235959Z");
        let id = b.next_id();
        b.push(
            id,
            "cal-fitness",
            "Gym",
            gym,
            Some(Recurrence {
                rrule: format!("FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL={until}"),
                ..Recurrence::default()
            }),
            None,
        );

        // Weekly 1:1 with a moved occurrence (override row).
        let first_tue = weekday_on_or_after(window_start, chrono::Weekday::Tue);
        let one_on_one = b.zoned(first_tue, 14, 0, 30);
        let series_id = b.next_id();
        b.push(
            series_id.clone(),
            "cal-work",
            "1:1 with manager",
            one_on_one,
            Some(Recurrence {
                rrule: "FREQ=WEEKLY;COUNT=52".into(),
                ..Recurrence::default()
            }),
            None,
        );
        let moved_day = weekday_on_or_after(anchor, chrono::Weekday::Tue);
        let EventTiming::Zoned {
            start: original, ..
        } = b.zoned(moved_day, 14, 0, 30)
        else {
            unreachable!()
        };
        let key = OccurrenceKey::Instant(original);
        let moved = b.zoned(moved_day, 16, 0, 30);
        b.push(
            occurrence_id(&series_id, &key),
            "cal-work",
            "1:1 with manager (moved)",
            moved,
            None,
            Some(SeriesLink {
                series_id,
                recurrence_id: key,
            }),
        );

        // Sunday family dinner, unbounded.
        let dinner = b.zoned(
            weekday_on_or_after(window_start, chrono::Weekday::Sun),
            19,
            0,
            90,
        );
        let id = b.next_id();
        b.push(
            id,
            "cal-family",
            "Family dinner",
            dinner,
            Some(Recurrence {
                rrule: "FREQ=WEEKLY".into(),
                ..Recurrence::default()
            }),
            None,
        );

        // Yearly all-day birthdays.
        for i in 0..3 {
            let day = anchor + Duration::days(37 * i - 40);
            let name = NAMES[i as usize];
            let id = b.next_id();
            b.push(
                id,
                "cal-birthdays",
                &format!("{name}'s birthday"),
                EventTiming::AllDay {
                    start_date: day - Duration::days(365 * 3),
                    end_date: day - Duration::days(365 * 3) + Duration::days(1),
                },
                Some(Recurrence {
                    rrule: "FREQ=YEARLY".into(),
                    ..Recurrence::default()
                }),
                None,
            );
        }
    }

    let remaining = n - b.events.len();
    let all_day = (remaining as f64 * 0.08).round() as usize;
    let burst = (remaining as f64 * 0.12).round() as usize;
    let scattered = remaining - all_day - burst;

    // Busy days with overlapping events.
    let busy_days = burst.div_ceil(EVENTS_PER_BUSY_DAY).max(1) as i64;
    for i in 0..burst as i64 {
        let day_index = i % busy_days;
        let offset = (day_index * 7919) % (2 * BUSY_DAY_SPREAD + 1) - BUSY_DAY_SPREAD;
        let (calendar_id, topics) = *b.rng.pick(&TIMED_TOPICS);
        let title = *b.rng.pick(topics);
        let hour = b.rng.int(9, 16) as u32;
        let minute = *b.rng.pick(&[0u32, 15, 30, 45]);
        let minutes = *b.rng.pick(&[30i64, 45, 60, 90]);
        let timing = b.zoned(anchor + Duration::days(offset), hour, minute, minutes);
        let id = b.next_id();
        b.push(id, calendar_id, title, timing, None, None);
    }

    // Scattered timed events over the whole window.
    for _ in 0..scattered {
        let (calendar_id, topics) = *b.rng.pick(&TIMED_TOPICS);
        let title = *b.rng.pick(topics);
        let day = anchor + Duration::days(b.rng.int(-WINDOW_DAYS, WINDOW_DAYS));
        let hour = *b.rng.pick(&START_HOURS);
        let minute = *b.rng.pick(&[0u32, 15, 30, 45]);
        let minutes = *b.rng.pick(&DURATIONS_MIN);
        let timing = b.zoned(day, hour, minute, minutes);
        let id = b.next_id();
        b.push(id, calendar_id, title, timing, None, None);
    }

    // All-day and multi-day events.
    for _ in 0..all_day {
        let start = anchor + Duration::days(b.rng.int(-WINDOW_DAYS, WINDOW_DAYS));
        let roll = b.rng.next();
        let (calendar_id, title, days): (&str, String, i64) = if roll < 0.3 {
            ("cal-holidays", b.rng.pick(&HOLIDAYS).to_string(), 1)
        } else if roll < 0.65 {
            let t = *b
                .rng
                .pick(&["Conference", "Offsite", "Training", "Deadline"]);
            ("cal-work", t.into(), b.rng.int(1, 4))
        } else {
            let t = *b.rng.pick(&["Vacation", "Trip", "Day off", "Moving day"]);
            ("cal-personal", t.into(), b.rng.int(1, 5))
        };
        let id = b.next_id();
        b.push(
            id,
            calendar_id,
            &title,
            EventTiming::AllDay {
                start_date: start,
                end_date: start + Duration::days(days),
            },
            None,
            None,
        );
    }

    Dataset {
        accounts: accounts(),
        calendars: calendars(),
        events: b.events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recurrence;

    fn options(count: usize) -> DatasetOptions {
        DatasetOptions {
            seed: 42,
            event_count: count,
            anchor: NaiveDate::from_ymd_opt(2026, 6, 15).unwrap(),
            time_zone: chrono_tz::Europe::Berlin,
        }
    }

    #[test]
    fn is_deterministic_with_exact_counts() {
        for size in DATASET_SIZES {
            let a = generate(&options(size));
            let b = generate(&options(size));
            assert_eq!(a.events, b.events);
            assert_eq!(a.events.len(), size);
            let ids: std::collections::HashSet<_> = a.events.iter().map(|e| &e.id).collect();
            assert_eq!(ids.len(), size);
        }
    }

    #[test]
    fn contains_valid_series_and_overrides() {
        let data = generate(&options(1000));
        let masters: Vec<_> = data
            .events
            .iter()
            .filter(|e| e.recurrence.is_some())
            .collect();
        assert_eq!(masters.len(), 7);
        for m in &masters {
            recurrence::validate(&m.timing, m.recurrence.as_ref().unwrap()).unwrap();
        }
        let overrides: Vec<_> = data.events.iter().filter(|e| e.series.is_some()).collect();
        assert_eq!(overrides.len(), 1);
        let link = overrides[0].series.as_ref().unwrap();
        let master = masters.iter().find(|m| m.id == link.series_id).unwrap();
        assert!(
            recurrence::has_occurrence(
                &master.timing,
                master.recurrence.as_ref().unwrap(),
                &link.recurrence_id,
                &chrono_tz::Europe::Berlin
            )
            .unwrap()
        );
        assert!(data.events.iter().all(|e| e.timing.validate().is_ok()));
    }

    #[test]
    fn small_datasets_skip_series() {
        let data = generate(&options(10));
        assert_eq!(data.events.len(), 10);
        assert!(data.events.iter().all(|e| e.recurrence.is_none()));
    }
}
