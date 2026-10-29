//! Identifies one occurrence of a series by its original start
//! (iCalendar RECURRENCE-ID / EXDATE / RDATE value).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use crate::error::CoreError;

/// Original start of an occurrence, in the form matching the series timing.
///
/// String form follows iCalendar basic format: `20261029T083000Z` (instant),
/// `20261029T093000` (floating wall time) or `20261029` (date).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OccurrenceKey {
    Instant(DateTime<Utc>),
    Floating(NaiveDateTime),
    Date(NaiveDate),
}

impl OccurrenceKey {
    /// The key moved by `delta` (dates move by whole days).
    pub fn shifted(&self, delta: chrono::Duration) -> Self {
        match self {
            Self::Instant(t) => Self::Instant(*t + delta),
            Self::Floating(t) => Self::Floating(*t + delta),
            Self::Date(d) => Self::Date(*d + chrono::Duration::days(delta.num_days())),
        }
    }
}

const INSTANT_FMT: &str = "%Y%m%dT%H%M%SZ";
const FLOATING_FMT: &str = "%Y%m%dT%H%M%S";
const DATE_FMT: &str = "%Y%m%d";

impl fmt::Display for OccurrenceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instant(t) => write!(f, "{}", t.format(INSTANT_FMT)),
            Self::Floating(t) => write!(f, "{}", t.format(FLOATING_FMT)),
            Self::Date(d) => write!(f, "{}", d.format(DATE_FMT)),
        }
    }
}

impl FromStr for OccurrenceKey {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || CoreError::InvalidOccurrenceKey(s.to_owned());
        match s.len() {
            16 if s.ends_with('Z') => NaiveDateTime::parse_from_str(&s[..15], FLOATING_FMT)
                .map(|t| Self::Instant(t.and_utc()))
                .map_err(|_| invalid()),
            15 => NaiveDateTime::parse_from_str(s, FLOATING_FMT)
                .map(Self::Floating)
                .map_err(|_| invalid()),
            8 => NaiveDate::parse_from_str(s, DATE_FMT)
                .map(Self::Date)
                .map_err(|_| invalid()),
            _ => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn round_trips_all_forms() {
        let keys = [
            OccurrenceKey::Instant(Utc.with_ymd_and_hms(2026, 10, 29, 8, 30, 0).unwrap()),
            OccurrenceKey::Floating(
                NaiveDate::from_ymd_opt(2026, 3, 29)
                    .unwrap()
                    .and_hms_opt(9, 30, 0)
                    .unwrap(),
            ),
            OccurrenceKey::Date(NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()),
        ];
        for key in keys {
            assert_eq!(key.to_string().parse::<OccurrenceKey>().unwrap(), key);
        }
        assert_eq!(keys[0].to_string(), "20261029T083000Z");
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "2026", "20261329", "20261029T250000Z", "yesterday"] {
            assert!(bad.parse::<OccurrenceKey>().is_err(), "{bad}");
        }
    }
}
