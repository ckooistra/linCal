//! SQLite persistence for the calendar engine.
//!
//! The database is the source of truth for displayed events. Queries are
//! limited to a time range; recurring series are expanded for that range
//! only, on Tokio's blocking pool so async workers (and the UI bridge
//! above them) are never stalled. Related writes share one transaction.

mod codec;
mod remote;

pub use remote::{
    AccountStatus, ApplyReport, IncomingObject, OpFailure, OpKind, PendingOp, Presence,
    ProblemOperation, PushObject, RemoteAccount, RemoteCalendarSpec, RemoteCalendarState,
    Resolution,
};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration as StdDuration;

use chrono::Utc;
use chrono_tz::Tz;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use thiserror::Error;

use calendar_core::recurrence::{self, KeyShift, retime_series};
use calendar_core::timing::MAX_ZONE_OFFSET;
use calendar_core::{
    Account, Calendar, CalendarEvent, CoreError, EventDraft, Occurrence, OccurrenceKey,
    ProviderKind, Recurrence, SeriesLink, SyncStatus, TimeRange, occurrence_id,
    parse_occurrence_id,
};

use codec::{EventRow, decode_event, encode_keys, encode_timing, event_columns, index_range};

/// Errors returned by [`Store`].
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Invalid(#[from] CoreError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("the calendar “{0}” is read-only")]
    ReadOnly(String),
    #[error("unknown calendar: {0}")]
    UnknownCalendar(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("stored data is corrupt: {0}")]
    Corrupt(String),
    #[error("a background task failed")]
    Task,
}

pub type Result<T> = std::result::Result<T, StorageError>;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Handle to the calendar database. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    /// Bounds concurrent decode/expansion jobs: rapid navigation must not
    /// pile up CPU work for ranges nobody looks at any more.
    expansions: std::sync::Arc<tokio::sync::Semaphore>,
}

/// Concurrent range expansions (visible range plus prefetch, with margin).
const MAX_EXPANSIONS: usize = 2;

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Current time at the millisecond precision the database stores.
fn now() -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp_millis(now_ms()).unwrap_or_default()
}

fn map_unique(error: sqlx::Error, id: &str) -> StorageError {
    match &error {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            StorageError::AlreadyExists(id.to_owned())
        }
        _ => StorageError::Database(error),
    }
}

impl Store {
    /// Opens (creating if needed) the database at `path` and applies migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        Self::connect(options, 4).await
    }

    /// Private in-memory database, for tests and benchmarks.
    pub async fn open_in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?;
        // One connection: every in-memory connection is a separate database.
        Self::connect(options, 1).await
    }

    async fn connect(options: SqliteConnectOptions, max: u32) -> Result<Self> {
        let options = options
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(max)
            .connect_with(options)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self {
            pool,
            expansions: std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_EXPANSIONS)),
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn is_empty(&self) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&self.pool)
            .await?;
        Ok(n == 0)
    }

    pub async fn event_count(&self) -> Result<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&self.pool)
            .await?)
    }

    // --- accounts and calendars ------------------------------------------

    pub async fn list_accounts(&self) -> Result<Vec<Account>> {
        let rows = sqlx::query("SELECT id, name, provider FROM accounts ORDER BY created_at, id")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                let provider: String = r.get("provider");
                Ok(Account {
                    id: r.get("id"),
                    name: r.get("name"),
                    provider: ProviderKind::parse(&provider)
                        .ok_or_else(|| StorageError::Corrupt(format!("provider: {provider}")))?,
                })
            })
            .collect()
    }

    pub async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        let rows = sqlx::query(
            "SELECT id, account_id, name, color, read_only FROM calendars ORDER BY sort_order, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| Calendar {
                id: r.get("id"),
                account_id: r.get("account_id"),
                name: r.get("name"),
                color: r.get("color"),
                read_only: r.get::<i64, _>("read_only") != 0,
            })
            .collect())
    }

    /// Sets a calendar's display colour (`#rrggbb`).
    pub async fn set_calendar_color(&self, calendar_id: &str, color: &str) -> Result<()> {
        let valid = color.len() == 7
            && color.starts_with('#')
            && color[1..].chars().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return Err(StorageError::Unsupported("colours must be #rrggbb"));
        }
        let done = sqlx::query("UPDATE calendars SET color = ? WHERE id = ?")
            .bind(color)
            .bind(calendar_id)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StorageError::UnknownCalendar(calendar_id.to_owned()));
        }
        Ok(())
    }

    // --- queries ----------------------------------------------------------

    pub async fn get_event(&self, id: &str) -> Result<Option<CalendarEvent>> {
        let row: Option<EventRow> = sqlx::query_as(concat!(
            "SELECT ",
            event_columns!(),
            " FROM events WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(decode_event).transpose()
    }

    /// Occurrences overlapping `range` as seen from `display_tz`, sorted by start.
    pub async fn query_range(&self, range: TimeRange, display_tz: Tz) -> Result<Vec<Occurrence>> {
        let slack = MAX_ZONE_OFFSET.num_milliseconds();
        let rows: Vec<EventRow> = sqlx::query_as(concat!(
            "SELECT ",
            event_columns!(),
            " FROM events WHERE range_start_ms < ? AND range_end_ms > ?"
        ))
        .bind(range.end.timestamp_millis().saturating_add(slack))
        .bind(range.start.timestamp_millis().saturating_sub(slack))
        .fetch_all(&self.pool)
        .await?;

        let master_ids: Vec<&str> = rows
            .iter()
            .filter(|r| r.rrule.is_some())
            .map(|r| r.id.as_str())
            .collect();
        let mut overridden: HashMap<String, HashSet<OccurrenceKey>> = HashMap::new();
        if !master_ids.is_empty() {
            let ids = serde_json::to_string(&master_ids).expect("strings serialize");
            let pairs = sqlx::query(
                "SELECT series_id, recurrence_id FROM events \
                 WHERE series_id IN (SELECT value FROM json_each(?))",
            )
            .bind(ids)
            .fetch_all(&self.pool)
            .await?;
            for pair in pairs {
                let series: String = pair.get("series_id");
                let key: String = pair.get("recurrence_id");
                let key = key
                    .parse()
                    .map_err(|_| StorageError::Corrupt(format!("recurrence id: {key}")))?;
                overridden.entry(series).or_default().insert(key);
            }
        }

        // Decoding and recurrence expansion are CPU work: keep them off the
        // async workers, a bounded number at a time (further requests wait
        // instead of competing for every core).
        let permit = std::sync::Arc::clone(&self.expansions)
            .acquire_owned()
            .await
            .map_err(|_| StorageError::Task)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let empty = HashSet::new();
            let mut result = Vec::new();
            for row in rows {
                let event = decode_event(row)?;
                let skip = overridden.get(&event.id).unwrap_or(&empty);
                result.extend(recurrence::expand(&event, &range, &display_tz, skip)?);
            }
            result.sort_by_key(|o| o.timing.bounds(&display_tz).start);
            Ok(result)
        })
        .await
        .map_err(|_| StorageError::Task)?
    }

    // --- writes -----------------------------------------------------------

    async fn writable_calendar(tx: &mut Transaction<'_, Sqlite>, calendar_id: &str) -> Result<()> {
        let row = sqlx::query("SELECT name, read_only FROM calendars WHERE id = ?")
            .bind(calendar_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| StorageError::UnknownCalendar(calendar_id.to_owned()))?;
        if row.get::<i64, _>("read_only") != 0 {
            return Err(StorageError::ReadOnly(row.get("name")));
        }
        Ok(())
    }

    async fn load(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<CalendarEvent> {
        let row: EventRow = sqlx::query_as(concat!(
            "SELECT ",
            event_columns!(),
            " FROM events WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| StorageError::NotFound(id.to_owned()))?;
        decode_event(row)
    }

    async fn insert(tx: &mut Transaction<'_, Sqlite>, event: &CalendarEvent) -> Result<()> {
        let timing = encode_timing(&event.timing);
        let (range_start, range_end) = index_range(event)?;
        let (rrule, rdates, exdates) = match &event.recurrence {
            Some(r) => (
                Some(r.rrule.as_str()),
                encode_keys(&r.rdates),
                encode_keys(&r.exdates),
            ),
            None => (None, String::new(), String::new()),
        };
        sqlx::query(concat!(
            "INSERT INTO events (",
            event_columns!(),
            ", range_start_ms, range_end_ms) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(&event.id)
        .bind(&event.calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(timing.kind)
        .bind(timing.start_value)
        .bind(timing.end_value)
        .bind(timing.time_zone)
        .bind(rrule)
        .bind(rdates)
        .bind(exdates)
        .bind(event.series.as_ref().map(|s| s.series_id.as_str()))
        .bind(event.series.as_ref().map(|s| s.recurrence_id.to_string()))
        .bind(event.sync_status.as_str())
        .bind(event.revision)
        .bind(event.created_at.timestamp_millis())
        .bind(event.updated_at.timestamp_millis())
        .bind(range_start)
        .bind(range_end)
        .execute(&mut **tx)
        .await
        .map_err(|e| map_unique(e, &event.id))?;
        Ok(())
    }

    async fn delete_row(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM events WHERE id = ?")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Rewrites every column of an existing row.
    async fn update_row(tx: &mut Transaction<'_, Sqlite>, event: &CalendarEvent) -> Result<()> {
        let timing = encode_timing(&event.timing);
        let (range_start, range_end) = index_range(event)?;
        let (rrule, rdates, exdates) = match &event.recurrence {
            Some(r) => (
                Some(r.rrule.as_str()),
                encode_keys(&r.rdates),
                encode_keys(&r.exdates),
            ),
            None => (None, String::new(), String::new()),
        };
        let done = sqlx::query(
            "UPDATE events SET calendar_id = ?, title = ?, description = ?, location = ?, \
             timing_kind = ?, start_value = ?, end_value = ?, time_zone = ?, rrule = ?, rdates = ?, \
             exdates = ?, sync_status = ?, revision = ?, updated_at = ?, range_start_ms = ?, \
             range_end_ms = ? WHERE id = ?",
        )
        .bind(&event.calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(timing.kind)
        .bind(timing.start_value)
        .bind(timing.end_value)
        .bind(timing.time_zone)
        .bind(rrule)
        .bind(rdates)
        .bind(exdates)
        .bind(event.sync_status.as_str())
        .bind(event.revision)
        .bind(event.updated_at.timestamp_millis())
        .bind(range_start)
        .bind(range_end)
        .bind(&event.id)
        .execute(&mut **tx)
        .await?;
        if done.rows_affected() == 0 {
            return Err(StorageError::NotFound(event.id.clone()));
        }
        Ok(())
    }

    /// Moves the recurrence ids (and occurrence-derived row ids) of a
    /// series' overrides by `delta`, after the series start moved.
    async fn shift_overrides(
        tx: &mut Transaction<'_, Sqlite>,
        series_id: &str,
        shift: &KeyShift,
    ) -> Result<()> {
        if shift.delta.is_zero() {
            return Ok(());
        }
        // Ids are derived from recurrence ids; move the furthest ones first so
        // a shifted id never collides with one not yet moved.
        let sql = if shift.is_forward() {
            "SELECT id, recurrence_id FROM events WHERE series_id = ? ORDER BY range_start_ms DESC"
        } else {
            "SELECT id, recurrence_id FROM events WHERE series_id = ? ORDER BY range_start_ms ASC"
        };
        let rows = sqlx::query(sql)
            .bind(series_id)
            .fetch_all(&mut **tx)
            .await?;
        for row in rows {
            let id: String = row.get("id");
            let key: String = row.get("recurrence_id");
            let key: OccurrenceKey = key
                .parse()
                .map_err(|_| StorageError::Corrupt(format!("recurrence id: {key}")))?;
            let moved = shift.apply(&key);
            // base_recurrence_id remembers the id the server copy uses.
            sqlx::query(
                "UPDATE events SET id = ?, recurrence_id = ?, \
                 base_recurrence_id = COALESCE(base_recurrence_id, recurrence_id) WHERE id = ?",
            )
            .bind(occurrence_id(series_id, &moved))
            .bind(moved.to_string())
            .bind(&id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }

    /// Deletes overrides of a series for slots its (edited) rule no longer
    /// generates, e.g. after COUNT was reduced.
    async fn prune_overrides(
        tx: &mut Transaction<'_, Sqlite>,
        series_id: &str,
        timing: &calendar_core::EventTiming,
        rec: &Recurrence,
    ) -> Result<()> {
        let rows = sqlx::query("SELECT id, recurrence_id FROM events WHERE series_id = ?")
            .bind(series_id)
            .fetch_all(&mut **tx)
            .await?;
        for row in rows {
            let key: String = row.get("recurrence_id");
            let key: OccurrenceKey = key
                .parse()
                .map_err(|_| StorageError::Corrupt(format!("recurrence id: {key}")))?;
            if !recurrence::has_occurrence(timing, rec, &key, &chrono_tz::UTC)? {
                Self::delete_row(tx, &row.get::<String, _>("id")).await?;
            }
        }
        Ok(())
    }

    /// Events of synchronized calendars cannot change calendar yet (that
    /// needs a server-side move or delete + create).
    async fn check_move(tx: &mut Transaction<'_, Sqlite>, from: &str, to: &str) -> Result<()> {
        if from != to
            && (Self::calendar_remote(tx, from).await?.is_some()
                || Self::calendar_remote(tx, to).await?.is_some())
        {
            return Err(StorageError::Unsupported(
                "moving events to or from a synchronized calendar is not supported yet",
            ));
        }
        Ok(())
    }

    /// Marks the object of `event_id` as changed locally, if it is remote,
    /// and returns the sync status its rows now have.
    async fn touch_event(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        event_id: &str,
    ) -> Result<Option<SyncStatus>> {
        match Self::event_href(tx, event_id).await? {
            Some(href) => Ok(Some(Self::touch_object(tx, calendar_id, &href).await?)),
            None => Ok(None),
        }
    }

    /// An RDATE-only recurrence without dates is no recurrence.
    fn meaningful(recurrence: Option<Recurrence>) -> Option<Recurrence> {
        recurrence.filter(|r| !(recurrence::is_rdate_only(r) && r.rdates.is_empty()))
    }

    fn validated(draft: &EventDraft) -> Result<Option<Recurrence>> {
        draft.validate()?;
        Ok(match &draft.rrule {
            Some(rrule) => {
                let rec = Recurrence {
                    rrule: rrule.clone(),
                    ..Recurrence::default()
                };
                recurrence::validate(&draft.timing, &rec)?;
                Some(rec)
            }
            None => None,
        })
    }

    /// Creates an event (optionally recurring) with a caller-chosen stable id.
    pub async fn create_event(&self, id: &str, draft: EventDraft) -> Result<CalendarEvent> {
        if id.is_empty() || id.contains(calendar_core::model::OCCURRENCE_SEPARATOR) {
            return Err(StorageError::Unsupported("invalid event id"));
        }
        let recurrence = Self::meaningful(Self::validated(&draft)?);
        let now = now();
        let event = CalendarEvent {
            id: id.to_owned(),
            calendar_id: draft.calendar_id,
            title: draft.title,
            description: draft.description,
            location: draft.location,
            timing: draft.timing,
            recurrence,
            series: None,
            sync_status: SyncStatus::Saved,
            revision: 1,
            created_at: now,
            updated_at: now,
        };
        let mut event = event;
        let mut tx = self.pool.begin().await?;
        Self::writable_calendar(&mut tx, &event.calendar_id).await?;
        Self::insert(&mut tx, &event).await?;
        if let Some(url) = Self::calendar_remote(&mut tx, &event.calendar_id).await? {
            Self::attach_new_object(&mut tx, &event.calendar_id, &url, &event.id).await?;
            event.sync_status = SyncStatus::PendingSync;
        }
        tx.commit().await?;
        Ok(event)
    }

    /// Updates a stored row by id: a one-off event, an override, or a whole
    /// series addressed by its master id.
    pub async fn update_event(&self, id: &str, draft: EventDraft) -> Result<CalendarEvent> {
        let mut tx = self.pool.begin().await?;
        let current = Self::load(&mut tx, id).await?;
        Self::writable_calendar(&mut tx, &current.calendar_id).await?;
        Self::writable_calendar(&mut tx, &draft.calendar_id).await?;
        if current.series.is_some() && draft.calendar_id != current.calendar_id {
            return Err(StorageError::Unsupported(
                "a single occurrence cannot move to another calendar",
            ));
        }
        Self::check_move(&mut tx, &current.calendar_id, &draft.calendar_id).await?;
        let recurrence = if current.series.is_some() {
            draft.validate()?;
            None
        } else {
            let mut rec = Self::validated(&draft)?;
            // Keep RDATE/EXDATE when the rule itself is unchanged.
            if let (Some(new), Some(old)) = (&mut rec, &current.recurrence)
                && new.rrule == old.rrule
                && draft.timing == current.timing
            {
                new.rdates.clone_from(&old.rdates);
                new.exdates.clone_from(&old.exdates);
            }
            Self::meaningful(rec)
        };
        if current.recurrence.is_some() && (recurrence.is_none() || draft.timing != current.timing)
        {
            // The series' occurrences changed: their overrides no longer apply.
            sqlx::query("DELETE FROM events WHERE series_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        } else if let Some(rec) = &recurrence
            && current.recurrence.as_ref() != Some(rec)
        {
            Self::prune_overrides(&mut tx, id, &draft.timing, rec).await?;
        }
        let updated = CalendarEvent {
            calendar_id: draft.calendar_id,
            title: draft.title,
            description: draft.description,
            location: draft.location,
            timing: draft.timing,
            recurrence,
            sync_status: SyncStatus::Saved,
            revision: current.revision + 1,
            updated_at: now(),
            ..current
        };
        let mut updated = updated;
        Self::update_row(&mut tx, &updated).await?;
        if let Some(status) = Self::touch_event(&mut tx, &updated.calendar_id, id).await? {
            updated.sync_status = status;
        }
        tx.commit().await?;
        Ok(updated)
    }

    /// Applies `draft` to the whole series that `occurrence_id` belongs to.
    /// The series start moves by the same offset as the edited occurrence;
    /// cancelled and modified occurrences move with it. Choosing "does not
    /// repeat" turns the series into this single occurrence.
    pub async fn update_series(
        &self,
        occurrence: &str,
        draft: EventDraft,
    ) -> Result<CalendarEvent> {
        let (series_id, key) = parse_occurrence_id(occurrence)
            .ok_or_else(|| StorageError::NotFound(occurrence.to_owned()))?;
        let series_id = series_id.to_owned();
        let mut tx = self.pool.begin().await?;
        let master = Self::load(&mut tx, &series_id).await?;
        let Some(old_rec) = master.recurrence.clone() else {
            return Err(StorageError::NotFound(occurrence.to_owned()));
        };
        Self::writable_calendar(&mut tx, &master.calendar_id).await?;
        Self::writable_calendar(&mut tx, &draft.calendar_id).await?;
        Self::check_move(&mut tx, &master.calendar_id, &draft.calendar_id).await?;
        draft.validate()?;
        // If the edited occurrence was moved on its own, measure the change
        // from where it is displayed, not from its original slot.
        let edited_override = match Self::load(&mut tx, occurrence).await {
            Ok(row) => Some(row),
            Err(StorageError::NotFound(_)) => None,
            Err(e) => return Err(e),
        };
        let reference = edited_override
            .as_ref()
            .map_or(key, |o| o.timing.start_key());

        let (timing, recurrence, shift) = match &draft.rrule {
            None => (draft.timing.clone(), None, None),
            Some(rrule) => {
                let retimed = retime_series(&master.timing, &reference, &draft.timing);
                let mut rec = Recurrence {
                    rrule: rrule.clone(),
                    ..Recurrence::default()
                };
                if let Some(shift) = retimed.key_shift {
                    rec.rdates = old_rec.rdates.iter().map(|k| shift.apply(k)).collect();
                    rec.exdates = old_rec.exdates.iter().map(|k| shift.apply(k)).collect();
                }
                recurrence::validate(&retimed.timing, &rec)?;
                (
                    retimed.timing,
                    Self::meaningful(Some(rec)),
                    retimed.key_shift,
                )
            }
        };
        let keep_overrides = recurrence.is_some() && shift.is_some();
        if !keep_overrides {
            sqlx::query("DELETE FROM events WHERE series_id = ?")
                .bind(&series_id)
                .execute(&mut *tx)
                .await?;
        }
        if keep_overrides {
            // Overrides follow the calendar of their series, and take text
            // changes unless they had their own value for that field.
            sqlx::query("UPDATE events SET calendar_id = ? WHERE series_id = ?")
                .bind(&draft.calendar_id)
                .bind(&series_id)
                .execute(&mut *tx)
                .await?;
            for (column, old, new) in [
                ("title", &master.title, &draft.title),
                ("description", &master.description, &draft.description),
                ("location", &master.location, &draft.location),
            ] {
                if old == new {
                    continue;
                }
                let sql = match column {
                    "title" => "UPDATE events SET title = ? WHERE series_id = ? AND title = ?",
                    "description" => {
                        "UPDATE events SET description = ? WHERE series_id = ? AND description = ?"
                    }
                    _ => "UPDATE events SET location = ? WHERE series_id = ? AND location = ?",
                };
                sqlx::query(sql)
                    .bind(new)
                    .bind(&series_id)
                    .bind(old)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        let updated = CalendarEvent {
            calendar_id: draft.calendar_id,
            title: draft.title,
            description: draft.description,
            location: draft.location,
            timing,
            recurrence,
            sync_status: SyncStatus::Saved,
            revision: master.revision + 1,
            updated_at: now(),
            ..master
        };
        Self::update_row(&mut tx, &updated).await?;
        if let (true, Some(shift)) = (keep_overrides, shift) {
            if let Some(mut edited) = edited_override {
                // The occurrence the user edited takes the new details too.
                edited.title.clone_from(&updated.title);
                edited.description.clone_from(&updated.description);
                edited.location.clone_from(&updated.location);
                edited.timing = draft.timing.clone();
                edited.revision += 1;
                edited.updated_at = updated.updated_at;
                Self::update_row(&mut tx, &edited).await?;
            }
            Self::shift_overrides(&mut tx, &series_id, &shift).await?;
            if let Some(rec) = &updated.recurrence {
                Self::prune_overrides(&mut tx, &series_id, &updated.timing, rec).await?;
            }
        }
        let mut updated = updated;
        if let Some(status) = Self::touch_event(&mut tx, &updated.calendar_id, &series_id).await? {
            updated.sync_status = status;
        }
        tx.commit().await?;
        Ok(updated)
    }

    /// Changes a single occurrence of a series (creating or updating its
    /// override row). Returns the occurrence as it now appears.
    pub async fn update_occurrence(
        &self,
        occurrence: &str,
        draft: EventDraft,
        display_tz: Tz,
    ) -> Result<Occurrence> {
        let (series_id, key) = parse_occurrence_id(occurrence)
            .ok_or_else(|| StorageError::NotFound(occurrence.to_owned()))?;
        draft.validate()?;
        let mut tx = self.pool.begin().await?;
        let master = Self::load(&mut tx, series_id).await?;
        let Some(rec) = &master.recurrence else {
            return Err(StorageError::NotFound(occurrence.to_owned()));
        };
        Self::writable_calendar(&mut tx, &master.calendar_id).await?;
        if draft.calendar_id != master.calendar_id {
            return Err(StorageError::Unsupported(
                "a single occurrence cannot move to another calendar",
            ));
        }
        let now = now();
        let (event, exists) = match Self::load(&mut tx, occurrence).await {
            Ok(existing) => (
                CalendarEvent {
                    title: draft.title,
                    description: draft.description,
                    location: draft.location,
                    timing: draft.timing,
                    revision: existing.revision + 1,
                    updated_at: now,
                    sync_status: SyncStatus::Saved,
                    ..existing
                },
                true,
            ),
            Err(StorageError::NotFound(_)) => {
                if !recurrence::has_occurrence(&master.timing, rec, &key, &display_tz)? {
                    return Err(StorageError::NotFound(occurrence.to_owned()));
                }
                (
                    CalendarEvent {
                        id: occurrence.to_owned(),
                        calendar_id: master.calendar_id.clone(),
                        title: draft.title,
                        description: draft.description,
                        location: draft.location,
                        timing: draft.timing,
                        recurrence: None,
                        series: Some(SeriesLink {
                            series_id: series_id.to_owned(),
                            recurrence_id: key,
                        }),
                        sync_status: SyncStatus::Saved,
                        revision: master.revision + 1,
                        created_at: now,
                        updated_at: now,
                    },
                    false,
                )
            }
            Err(e) => return Err(e),
        };
        if exists {
            Self::update_row(&mut tx, &event).await?;
        } else {
            Self::insert(&mut tx, &event).await?;
            if let Some(href) = Self::event_href(&mut tx, series_id).await? {
                sqlx::query("UPDATE events SET remote_href = ? WHERE id = ?")
                    .bind(&href)
                    .bind(&event.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        let mut event = event;
        if let Some(status) = Self::touch_event(&mut tx, &event.calendar_id, series_id).await? {
            event.sync_status = status;
        }
        tx.commit().await?;
        let mut occ = event.as_occurrence();
        occ.rrule = Some(rec.rrule.clone());
        Ok(occ)
    }

    /// Deletes a stored event. Deleting a series master deletes the series
    /// including its modified occurrences.
    pub async fn delete_event(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let event = Self::load(&mut tx, id).await?;
        Self::writable_calendar(&mut tx, &event.calendar_id).await?;
        let href = Self::event_href(&mut tx, id).await?;
        Self::delete_row(&mut tx, id).await?;
        if let Some(href) = href {
            Self::object_changed_or_removed(&mut tx, &event.calendar_id, &href).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Deletes the whole series that `occurrence` belongs to.
    pub async fn delete_series(&self, occurrence: &str) -> Result<()> {
        let (series_id, _) = parse_occurrence_id(occurrence)
            .ok_or_else(|| StorageError::NotFound(occurrence.to_owned()))?;
        self.delete_event(series_id).await
    }

    /// Cancels one occurrence of a series (EXDATE) and drops its override.
    pub async fn delete_occurrence(&self, occurrence: &str) -> Result<()> {
        let (series_id, key) = parse_occurrence_id(occurrence)
            .ok_or_else(|| StorageError::NotFound(occurrence.to_owned()))?;
        let mut tx = self.pool.begin().await?;
        let mut master = Self::load(&mut tx, series_id).await?;
        Self::writable_calendar(&mut tx, &master.calendar_id).await?;
        let Some(rec) = master.recurrence.as_mut() else {
            return Err(StorageError::NotFound(occurrence.to_owned()));
        };
        if !rec.exdates.contains(&key) {
            rec.exdates.push(key);
            rec.exdates.sort();
        }
        sqlx::query(
            "UPDATE events SET exdates = ?, revision = revision + 1, updated_at = ? WHERE id = ?",
        )
        .bind(encode_keys(&rec.exdates))
        .bind(now_ms())
        .bind(series_id)
        .execute(&mut *tx)
        .await?;
        Self::delete_row(&mut tx, occurrence).await?;
        Self::touch_event(&mut tx, &master.calendar_id, series_id).await?;
        tx.commit().await?;
        master.revision += 1;
        Ok(())
    }

    /// Replaces all *simulated* accounts, calendars and events in one
    /// transaction (used to load simulated datasets). Real accounts are kept.
    pub async fn replace_all(
        &self,
        accounts: &[Account],
        calendars: &[Calendar],
        events: &[CalendarEvent],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Calendars and events go with their account (ON DELETE CASCADE).
        sqlx::query("DELETE FROM accounts WHERE provider = 'simulated'")
            .execute(&mut *tx)
            .await?;
        let now = now_ms();
        for (i, a) in accounts.iter().enumerate() {
            sqlx::query(
                "INSERT INTO accounts (id, name, provider, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(&a.id)
            .bind(&a.name)
            .bind(a.provider.as_str())
            .bind(now + i as i64)
            .execute(&mut *tx)
            .await?;
        }
        for (i, c) in calendars.iter().enumerate() {
            sqlx::query(
                "INSERT INTO calendars (id, account_id, name, color, read_only, sort_order) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&c.id)
            .bind(&c.account_id)
            .bind(&c.name)
            .bind(&c.color)
            .bind(i64::from(c.read_only))
            .bind(i as i64)
            .execute(&mut *tx)
            .await?;
        }
        // Masters before overrides so foreign keys resolve.
        let (overrides, rest): (Vec<_>, Vec<_>) = events.iter().partition(|e| e.series.is_some());
        for event in rest.into_iter().chain(overrides) {
            Self::insert(&mut tx, event).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // --- settings ---------------------------------------------------------

    pub async fn settings(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let rows = sqlx::query("SELECT key, value FROM application_settings")
            .fetch_all(&self.pool)
            .await?;
        let mut map = serde_json::Map::new();
        for r in rows {
            let value: String = r.get("value");
            if let Ok(v) = serde_json::from_str(&value) {
                map.insert(r.get("key"), v);
            }
        }
        Ok(map)
    }

    pub async fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        if key.is_empty() || key.len() > 128 {
            return Err(StorageError::Unsupported("invalid setting key"));
        }
        sqlx::query(
            "INSERT INTO application_settings (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
