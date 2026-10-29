//! Provider-neutral calendar model.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::occurrence_key::OccurrenceKey;

pub type AccountId = String;
pub type CalendarId = String;
pub type EventId = String;

/// Kinds of account the architecture plans for. Only `Local` and
/// `Simulated` exist before Milestone 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Local,
    Simulated,
    Caldav,
    Google,
    Microsoft,
    Ics,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Simulated => "simulated",
            Self::Caldav => "caldav",
            Self::Google => "google",
            Self::Microsoft => "microsoft",
            Self::Ics => "ics",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local" => Self::Local,
            "simulated" => Self::Simulated,
            "caldav" => Self::Caldav,
            "google" => Self::Google,
            "microsoft" => Self::Microsoft,
            "ics" => Self::Ics,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: AccountId,
    pub name: String,
    pub provider: ProviderKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub id: CalendarId,
    pub account_id: AccountId,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    pub read_only: bool,
}

/// Lifecycle of a change (spec §4.3). Local-only data stops at `Saved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncStatus {
    PendingLocal,
    Saved,
    PendingSync,
    Synced,
    SyncFailed,
    Conflict,
}

impl SyncStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingLocal => "pending-local",
            Self::Saved => "saved",
            Self::PendingSync => "pending-sync",
            Self::Synced => "synced",
            Self::SyncFailed => "sync-failed",
            Self::Conflict => "conflict",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending-local" => Self::PendingLocal,
            "saved" => Self::Saved,
            "pending-sync" => Self::PendingSync,
            "synced" => Self::Synced,
            "sync-failed" => Self::SyncFailed,
            "conflict" => Self::Conflict,
            _ => return None,
        })
    }
}

/// When an event happens.
///
/// * `Zoned`: wall-clock start/end in an IANA zone, stored as the UTC
///   instants they denote. Recurrences expand in that zone, so a 09:30
///   meeting stays at 09:30 across DST.
/// * `Floating`: wall-clock time with no zone (iCalendar "floating"); it
///   happens at that local time wherever the viewer is.
/// * `AllDay`: calendar dates, end exclusive; never converted through a zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventTiming {
    Zoned {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        time_zone: chrono_tz::Tz,
    },
    Floating {
        start: NaiveDateTime,
        end: NaiveDateTime,
    },
    AllDay {
        start_date: NaiveDate,
        end_date: NaiveDate,
    },
}

/// Recurrence rule set of a series master (RFC 5545 RRULE/RDATE/EXDATE).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recurrence {
    /// RRULE value without the `RRULE:` prefix, e.g. `FREQ=WEEKLY;BYDAY=MO`.
    pub rrule: String,
    pub rdates: Vec<OccurrenceKey>,
    pub exdates: Vec<OccurrenceKey>,
}

/// Links an override (a modified single occurrence) to its series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesLink {
    pub series_id: EventId,
    /// Original start of the occurrence this row replaces (RECURRENCE-ID).
    pub recurrence_id: OccurrenceKey,
}

/// A stored event: a one-off event, a series master, or an override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    pub id: EventId,
    pub calendar_id: CalendarId,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing: EventTiming,
    pub recurrence: Option<Recurrence>,
    pub series: Option<SeriesLink>,
    pub sync_status: SyncStatus,
    /// Incremented on every local change.
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The user-editable fields of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    pub calendar_id: CalendarId,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing: EventTiming,
    /// RRULE for a series (`None` = not recurring). Ignored when editing a
    /// single occurrence.
    pub rrule: Option<String>,
}

/// One concrete appearance of an event inside a queried range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// Stable id: the event id, or `<series id>::<recurrence id>` for
    /// occurrences generated from a series.
    pub id: String,
    /// Stored row this occurrence comes from (master or override).
    pub event_id: EventId,
    /// Series this occurrence belongs to, if any.
    pub series_id: Option<EventId>,
    pub recurrence_id: Option<OccurrenceKey>,
    pub calendar_id: CalendarId,
    pub title: String,
    pub description: String,
    pub location: String,
    pub timing: EventTiming,
    /// RRULE of the series this occurrence was generated from.
    pub rrule: Option<String>,
    pub sync_status: SyncStatus,
    pub revision: i64,
}

impl EventTiming {
    /// The start of this timing as an occurrence key of the matching kind.
    pub fn start_key(&self) -> OccurrenceKey {
        match self {
            Self::Zoned { start, .. } => OccurrenceKey::Instant(*start),
            Self::Floating { start, .. } => OccurrenceKey::Floating(*start),
            Self::AllDay { start_date, .. } => OccurrenceKey::Date(*start_date),
        }
    }
}

/// Separator between series id and recurrence id in occurrence ids.
pub const OCCURRENCE_SEPARATOR: &str = "::";

pub fn occurrence_id(series_id: &str, key: &OccurrenceKey) -> String {
    format!("{series_id}{OCCURRENCE_SEPARATOR}{key}")
}

/// Splits an occurrence id into series id and recurrence id.
pub fn parse_occurrence_id(id: &str) -> Option<(&str, OccurrenceKey)> {
    let (series, key) = id.rsplit_once(OCCURRENCE_SEPARATOR)?;
    Some((series, key.parse().ok()?))
}

impl CalendarEvent {
    pub fn draft(&self) -> EventDraft {
        EventDraft {
            calendar_id: self.calendar_id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            timing: self.timing.clone(),
            rrule: self.recurrence.as_ref().map(|r| r.rrule.clone()),
        }
    }

    /// The event as a single (non-recurring) occurrence.
    pub fn as_occurrence(&self) -> Occurrence {
        Occurrence {
            id: self.id.clone(),
            event_id: self.id.clone(),
            series_id: self.series.as_ref().map(|s| s.series_id.clone()),
            recurrence_id: self.series.as_ref().map(|s| s.recurrence_id),
            calendar_id: self.calendar_id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            location: self.location.clone(),
            timing: self.timing.clone(),
            rrule: self.recurrence.as_ref().map(|r| r.rrule.clone()),
            sync_status: self.sync_status,
            revision: self.revision,
        }
    }
}
