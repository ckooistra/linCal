//! Converting event timings to instants and validating them.

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

use crate::error::CoreError;
use crate::model::{EventDraft, EventTiming};

/// Half-open UTC instant range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }

    pub fn overlaps(&self, other: &TimeRange) -> bool {
        self.start < other.end && self.end > other.start
    }
}

/// Maximum offset of any zone from UTC; used as slack when comparing
/// zone-independent values (dates, floating times) against instants.
pub const MAX_ZONE_OFFSET: Duration = Duration::hours(14);

pub const MAX_TITLE_CHARS: usize = 1024;

/// Resolve a wall-clock time in `tz`. Ambiguous times (DST fall-back) take
/// the earlier instant; times in a DST gap shift forward by the gap, as
/// RFC 5545 §3.3.5 prescribes.
pub fn resolve_local(tz: &Tz, local: NaiveDateTime) -> DateTime<Utc> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => t.with_timezone(&Utc),
        LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
        LocalResult::None => {
            // Interpret using the offset in effect just before the gap.
            let before = local - Duration::hours(3);
            let offset = match tz.from_local_datetime(&before) {
                LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => t.offset().fix(),
                LocalResult::None => return local.and_utc(),
            };
            (local - offset).and_utc()
        }
    }
}

/// Local midnight of `date` in `tz`, as an instant.
pub fn date_start(tz: &Tz, date: NaiveDate) -> DateTime<Utc> {
    resolve_local(tz, date.and_hms_opt(0, 0, 0).expect("midnight is valid"))
}

impl EventTiming {
    /// Instants covered by this timing for a viewer in `display_tz`.
    pub fn bounds(&self, display_tz: &Tz) -> TimeRange {
        match self {
            Self::Zoned { start, end, .. } => TimeRange::new(*start, *end),
            Self::Floating { start, end } => TimeRange::new(
                resolve_local(display_tz, *start),
                resolve_local(display_tz, *end),
            ),
            Self::AllDay {
                start_date,
                end_date,
            } => TimeRange::new(
                date_start(display_tz, *start_date),
                date_start(display_tz, *end_date),
            ),
        }
    }

    /// Zone-independent approximation used for indexing: zoned timings are
    /// exact; floating and all-day values are their wall time read as UTC
    /// and must be compared with [`MAX_ZONE_OFFSET`] slack.
    pub fn index_bounds(&self) -> TimeRange {
        match self {
            Self::Zoned { start, end, .. } => TimeRange::new(*start, *end),
            Self::Floating { start, end } => TimeRange::new(start.and_utc(), end.and_utc()),
            Self::AllDay {
                start_date,
                end_date,
            } => TimeRange::new(
                start_date.and_hms_opt(0, 0, 0).expect("valid").and_utc(),
                end_date.and_hms_opt(0, 0, 0).expect("valid").and_utc(),
            ),
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        let ok = match self {
            Self::Zoned { start, end, .. } => end > start,
            Self::Floating { start, end } => end > start,
            Self::AllDay {
                start_date,
                end_date,
            } => end_date > start_date,
        };
        if ok {
            Ok(())
        } else {
            Err(CoreError::EndBeforeStart)
        }
    }

    /// The same timing moved so that it starts `delta` later (all-day
    /// timings move by whole days, rounding toward zero).
    pub fn shifted(&self, delta: Duration) -> Self {
        match self {
            Self::Zoned {
                start,
                end,
                time_zone,
            } => Self::Zoned {
                start: *start + delta,
                end: *end + delta,
                time_zone: *time_zone,
            },
            Self::Floating { start, end } => Self::Floating {
                start: *start + delta,
                end: *end + delta,
            },
            Self::AllDay {
                start_date,
                end_date,
            } => {
                let days = Duration::days(delta.num_days());
                Self::AllDay {
                    start_date: *start_date + days,
                    end_date: *end_date + days,
                }
            }
        }
    }
}

pub fn parse_time_zone(name: &str) -> Result<Tz, CoreError> {
    name.parse::<Tz>()
        .map_err(|_| CoreError::UnknownTimeZone(name.to_owned()))
}

impl EventDraft {
    /// Validates user-editable fields. Calendar existence and write access
    /// are checked by storage, which knows the calendars.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return Err(CoreError::TitleTooLong);
        }
        if let Some(reminders) = &self.reminders
            && (reminders.len() > crate::model::MAX_REMINDERS
                || reminders
                    .iter()
                    .any(|m| *m > crate::model::MAX_REMINDER_MINUTES))
        {
            return Err(CoreError::InvalidReminders);
        }
        self.timing.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> Tz {
        chrono_tz::Europe::Berlin
    }

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    #[test]
    fn resolves_dst_gap_forward() {
        // 2026-03-29 02:30 does not exist in Berlin; it becomes 03:30 CEST = 01:30Z.
        let t = resolve_local(&berlin(), local(2026, 3, 29, 2, 30));
        assert_eq!(t, Utc.with_ymd_and_hms(2026, 3, 29, 1, 30, 0).unwrap());
    }

    #[test]
    fn resolves_ambiguous_time_to_earlier_instant() {
        // 2026-10-25 02:30 happens twice in Berlin; take the CEST one (00:30Z).
        let t = resolve_local(&berlin(), local(2026, 10, 25, 2, 30));
        assert_eq!(t, Utc.with_ymd_and_hms(2026, 10, 25, 0, 30, 0).unwrap());
    }

    #[test]
    fn all_day_bounds_follow_viewer_zone() {
        let timing = EventTiming::AllDay {
            start_date: NaiveDate::from_ymd_opt(2026, 3, 29).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2026, 3, 30).unwrap(),
        };
        let b = timing.bounds(&berlin());
        // The DST day is 23 hours long.
        assert_eq!(b.end - b.start, Duration::hours(23));
    }

    #[test]
    fn validates_order() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let bad = EventTiming::AllDay {
            start_date: d,
            end_date: d,
        };
        assert_eq!(bad.validate(), Err(CoreError::EndBeforeStart));
    }

    #[test]
    fn rejects_unknown_zones() {
        assert!(parse_time_zone("Europe/Berlin").is_ok());
        assert!(parse_time_zone("Mars/Olympus").is_err());
    }
}
