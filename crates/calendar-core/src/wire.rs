//! Serializable shapes exchanged with the UI.
//!
//! They mirror `apps/desktop/src/types/calendar.ts`. Instants are epoch
//! milliseconds; a timed value with `timeZone: null` is floating and is
//! resolved in the viewer's zone in both directions.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::{EventDraft, EventTiming, Occurrence, SyncStatus};
use crate::timing::{TimeRange, parse_time_zone, resolve_local};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum WireTiming {
    #[serde(rename = "timed", rename_all = "camelCase")]
    Timed {
        start: i64,
        end: i64,
        time_zone: Option<String>,
    },
    #[serde(rename = "all-day", rename_all = "camelCase")]
    AllDay {
        start_date: String,
        end_date: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRange {
    pub start: i64,
    pub end: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireDraft {
    pub calendar_id: String,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing: WireTiming,
    #[serde(default)]
    pub rrule: Option<String>,
}

/// An occurrence as the UI's `CalendarEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireEvent {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing: WireTiming,
    pub series_id: Option<String>,
    pub recurrence_id: Option<String>,
    pub rrule: Option<String>,
    pub sync_status: SyncStatus,
    pub revision: i64,
}

fn instant(ms: i64) -> Result<DateTime<Utc>, CoreError> {
    DateTime::from_timestamp_millis(ms).ok_or(CoreError::InvalidTime)
}

fn date(value: &str) -> Result<NaiveDate, CoreError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| CoreError::InvalidDate(value.to_owned()))
}

impl WireRange {
    pub fn to_range(self) -> Result<TimeRange, CoreError> {
        let range = TimeRange::new(instant(self.start)?, instant(self.end)?);
        if range.end <= range.start {
            return Err(CoreError::EndBeforeStart);
        }
        Ok(range)
    }
}

impl WireTiming {
    pub fn from_timing(timing: &EventTiming, display_tz: &Tz) -> Self {
        match timing {
            EventTiming::Zoned {
                start,
                end,
                time_zone,
            } => Self::Timed {
                start: start.timestamp_millis(),
                end: end.timestamp_millis(),
                time_zone: Some(time_zone.name().to_owned()),
            },
            EventTiming::Floating { start, end } => Self::Timed {
                start: resolve_local(display_tz, *start).timestamp_millis(),
                end: resolve_local(display_tz, *end).timestamp_millis(),
                time_zone: None,
            },
            EventTiming::AllDay {
                start_date,
                end_date,
            } => Self::AllDay {
                start_date: start_date.format("%Y-%m-%d").to_string(),
                end_date: end_date.format("%Y-%m-%d").to_string(),
            },
        }
    }

    pub fn to_timing(&self, display_tz: &Tz) -> Result<EventTiming, CoreError> {
        let timing = match self {
            Self::Timed {
                start,
                end,
                time_zone: Some(name),
            } => EventTiming::Zoned {
                start: instant(*start)?,
                end: instant(*end)?,
                time_zone: parse_time_zone(name)?,
            },
            Self::Timed {
                start,
                end,
                time_zone: None,
            } => EventTiming::Floating {
                start: instant(*start)?.with_timezone(display_tz).naive_local(),
                end: instant(*end)?.with_timezone(display_tz).naive_local(),
            },
            Self::AllDay {
                start_date,
                end_date,
            } => EventTiming::AllDay {
                start_date: date(start_date)?,
                end_date: date(end_date)?,
            },
        };
        timing.validate()?;
        Ok(timing)
    }
}

impl WireDraft {
    pub fn to_draft(&self, display_tz: &Tz) -> Result<EventDraft, CoreError> {
        let draft = EventDraft {
            calendar_id: self.calendar_id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            timing: self.timing.to_timing(display_tz)?,
            rrule: self.rrule.clone().filter(|r| !r.trim().is_empty()),
        };
        draft.validate()?;
        Ok(draft)
    }
}

impl WireEvent {
    pub fn from_occurrence(o: &Occurrence, display_tz: &Tz) -> Self {
        Self {
            id: o.id.clone(),
            calendar_id: o.calendar_id.clone(),
            title: o.title.clone(),
            description: o.description.clone(),
            location: o.location.clone(),
            timing: WireTiming::from_timing(&o.timing, display_tz),
            series_id: o.series_id.clone(),
            recurrence_id: o.recurrence_id.map(|k| k.to_string()),
            rrule: o.rrule.clone(),
            sync_status: o.sync_status,
            revision: o.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_like_the_typescript_model() {
        let t = WireTiming::Timed {
            start: 1,
            end: 2,
            time_zone: Some("Europe/Berlin".into()),
        };
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            r#"{"kind":"timed","start":1,"end":2,"timeZone":"Europe/Berlin"}"#
        );
        let a: WireTiming = serde_json::from_str(
            r#"{"kind":"all-day","startDate":"2026-01-01","endDate":"2026-01-02"}"#,
        )
        .unwrap();
        assert!(matches!(a, WireTiming::AllDay { .. }));
    }

    #[test]
    fn floating_round_trips_through_the_viewer_zone() {
        let tz = chrono_tz::America::New_York;
        let wire = WireTiming::Timed {
            start: 1_780_000_000_000,
            end: 1_780_003_600_000,
            time_zone: None,
        };
        let timing = wire.to_timing(&tz).unwrap();
        assert!(matches!(timing, EventTiming::Floating { .. }));
        assert_eq!(WireTiming::from_timing(&timing, &tz), wire);
    }

    #[test]
    fn rejects_bad_input() {
        let tz = chrono_tz::UTC;
        let backwards = WireTiming::Timed {
            start: 10,
            end: 5,
            time_zone: Some("UTC".into()),
        };
        assert_eq!(backwards.to_timing(&tz), Err(CoreError::EndBeforeStart));
        let zone = WireTiming::Timed {
            start: 1,
            end: 5,
            time_zone: Some("Nowhere/Land".into()),
        };
        assert!(matches!(
            zone.to_timing(&tz),
            Err(CoreError::UnknownTimeZone(_))
        ));
        assert!(WireRange { start: 5, end: 5 }.to_range().is_err());
    }
}
