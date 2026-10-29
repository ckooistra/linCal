//! Mapping between `calendar-core` types and SQLite rows.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use calendar_core::recurrence;
use calendar_core::{
    CalendarEvent, EventTiming, OccurrenceKey, Recurrence, SeriesLink, SyncStatus, parse_time_zone,
};

use crate::StorageError;

const FLOATING_FMT: &str = "%Y-%m-%dT%H:%M:%S";
const DATE_FMT: &str = "%Y-%m-%d";

/// Columns of the `events` table.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct EventRow {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing_kind: String,
    pub start_value: String,
    pub end_value: String,
    pub time_zone: Option<String>,
    pub rrule: Option<String>,
    pub rdates: String,
    pub exdates: String,
    pub series_id: Option<String>,
    pub recurrence_id: Option<String>,
    pub sync_status: String,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Column list matching [`EventRow`], as a literal so queries stay static.
macro_rules! event_columns {
    () => {
        "id, calendar_id, title, description, location, timing_kind, start_value, end_value, \
         time_zone, rrule, rdates, exdates, series_id, recurrence_id, sync_status, revision, \
         created_at, updated_at"
    };
}
pub(crate) use event_columns;

pub(crate) struct EncodedTiming {
    pub kind: &'static str,
    pub start_value: String,
    pub end_value: String,
    pub time_zone: Option<String>,
}

pub(crate) fn encode_timing(timing: &EventTiming) -> EncodedTiming {
    match timing {
        EventTiming::Zoned {
            start,
            end,
            time_zone,
        } => EncodedTiming {
            kind: "zoned",
            start_value: start.to_rfc3339(),
            end_value: end.to_rfc3339(),
            time_zone: Some(time_zone.name().to_owned()),
        },
        EventTiming::Floating { start, end } => EncodedTiming {
            kind: "floating",
            start_value: start.format(FLOATING_FMT).to_string(),
            end_value: end.format(FLOATING_FMT).to_string(),
            time_zone: None,
        },
        EventTiming::AllDay {
            start_date,
            end_date,
        } => EncodedTiming {
            kind: "all-day",
            start_value: start_date.format(DATE_FMT).to_string(),
            end_value: end_date.format(DATE_FMT).to_string(),
            time_zone: None,
        },
    }
}

fn corrupt(what: &str, value: &str) -> StorageError {
    StorageError::Corrupt(format!("{what}: {value}"))
}

fn instant(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| corrupt("instant", value))
}

fn floating(value: &str) -> Result<NaiveDateTime, StorageError> {
    NaiveDateTime::parse_from_str(value, FLOATING_FMT).map_err(|_| corrupt("local time", value))
}

fn date(value: &str) -> Result<NaiveDate, StorageError> {
    NaiveDate::parse_from_str(value, DATE_FMT).map_err(|_| corrupt("date", value))
}

pub(crate) fn decode_timing(row: &EventRow) -> Result<EventTiming, StorageError> {
    Ok(match row.timing_kind.as_str() {
        "zoned" => EventTiming::Zoned {
            start: instant(&row.start_value)?,
            end: instant(&row.end_value)?,
            time_zone: parse_time_zone(row.time_zone.as_deref().unwrap_or("UTC"))?,
        },
        "floating" => EventTiming::Floating {
            start: floating(&row.start_value)?,
            end: floating(&row.end_value)?,
        },
        "all-day" => EventTiming::AllDay {
            start_date: date(&row.start_value)?,
            end_date: date(&row.end_value)?,
        },
        other => return Err(corrupt("timing kind", other)),
    })
}

pub(crate) fn encode_keys(keys: &[OccurrenceKey]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn decode_keys(value: &str) -> Result<Vec<OccurrenceKey>, StorageError> {
    value
        .split_whitespace()
        .map(|k| k.parse().map_err(|_| corrupt("occurrence key", k)))
        .collect()
}

fn time_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

pub(crate) fn decode_event(row: EventRow) -> Result<CalendarEvent, StorageError> {
    let timing = decode_timing(&row)?;
    let recurrence = match row.rrule {
        Some(rrule) => Some(Recurrence {
            rrule,
            rdates: decode_keys(&row.rdates)?,
            exdates: decode_keys(&row.exdates)?,
        }),
        None => None,
    };
    let series = match (row.series_id, row.recurrence_id) {
        (Some(series_id), Some(key)) => Some(SeriesLink {
            series_id,
            recurrence_id: key.parse().map_err(|_| corrupt("recurrence id", &key))?,
        }),
        _ => None,
    };
    Ok(CalendarEvent {
        id: row.id,
        calendar_id: row.calendar_id,
        title: row.title,
        description: row.description,
        location: row.location,
        timing,
        recurrence,
        series,
        sync_status: SyncStatus::parse(&row.sync_status)
            .ok_or_else(|| corrupt("sync status", &row.sync_status))?,
        revision: row.revision,
        created_at: time_ms(row.created_at),
        updated_at: time_ms(row.updated_at),
    })
}

/// Zone-independent index bounds (epoch ms) of an event row.
pub(crate) fn index_range(event: &CalendarEvent) -> Result<(i64, i64), StorageError> {
    let bounds = event.timing.index_bounds();
    let end = match &event.recurrence {
        Some(rec) => recurrence::index_end(&event.timing, rec)?
            .map(|t| t.timestamp_millis())
            .unwrap_or(i64::MAX),
        None => bounds.end.timestamp_millis(),
    };
    Ok((bounds.start.timestamp_millis(), end))
}
