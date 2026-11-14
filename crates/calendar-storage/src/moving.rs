//! Moving an event (or a whole series) to another calendar.
//!
//! A move works for every provider, between accounts, and between local
//! and synchronized calendars, because it is a deletion plus a creation:
//! the rows are copied into the new calendar under a new id, and the
//! originals are deleted the usual way. In one transaction:
//!
//! * the old server copy, if there is one, is queued for deletion (with
//!   its version, so a change made there meanwhile becomes a conflict
//!   instead of being deleted);
//! * the copy, in a synchronized calendar, is queued for upload as a new
//!   object.
//!
//! The local rows are the source of truth throughout, so a failed upload
//! or deletion is retried and nothing is lost. The new id keeps the copy
//! apart from the old server copy, should that come back (a conflict
//! resolved by keeping the server's version).
//!
//! What the app doesn't model (attendees, conferencing) is not copied, as
//! with "this and following" splits: the server sees a new event.

use calendar_core::{
    CalendarEvent, EventDraft, SeriesLink, SyncStatus, occurrence_id, parse_occurrence_id,
};
use sqlx::{Sqlite, Transaction};

use crate::codec::{EventRow, decode_event, event_columns};
use crate::{Result, StorageError, Store, now};

impl Store {
    /// Saves an edit of the event `id` (a one-off event, or with an
    /// occurrence id the whole series, as [`Store::update_series`]) and, if
    /// `draft.calendar_id` names another calendar, moves it there under the
    /// id `new_id`. Returns the event as stored now.
    ///
    /// The new calendar is checked first (writable, what its provider can
    /// hold), so a move that would be refused leaves the event unchanged.
    pub async fn update_and_move(
        &self,
        id: &str,
        new_id: &str,
        mut draft: EventDraft,
    ) -> Result<CalendarEvent> {
        let series = parse_occurrence_id(id).is_some();
        let target = draft.calendar_id.clone();
        let from = {
            let mut tx = self.pool.begin().await?;
            let master = Self::master_of(&mut tx, id).await?;
            if master.calendar_id != target {
                Self::writable_calendar(&mut tx, &target).await?;
                let reminders = draft.reminders.as_ref().unwrap_or(&master.reminders);
                self.check_reminders(&mut tx, &target, Some(reminders))
                    .await?;
                if let Some(rrule) = draft.rrule.clone().filter(|_| series) {
                    let rec = calendar_core::Recurrence {
                        rrule,
                        ..calendar_core::Recurrence::default()
                    };
                    self.check_series_supported(&mut tx, &target, &draft.timing, &rec)
                        .await?;
                }
            }
            master.calendar_id
        };
        if from == target {
            return if series {
                self.update_series(id, draft).await
            } else {
                self.update_event(id, draft).await
            };
        }
        draft.calendar_id = from;
        let saved = if series {
            self.update_series(id, draft).await?
        } else {
            self.update_event(id, draft).await?
        };
        self.move_event(&saved.id, new_id, &target).await
    }

    /// Moves the event `id` to `calendar_id`. `id` may be a one-off event,
    /// a series master or any occurrence of a series: a series always moves
    /// as a whole, with its modified and cancelled occurrences. The moved
    /// event gets the id `new_id`; it is returned as stored now.
    ///
    /// Only the calendar changes. Callers apply other edits first (with
    /// [`Store::update_event`] or [`Store::update_series`]).
    pub async fn move_event(
        &self,
        id: &str,
        new_id: &str,
        calendar_id: &str,
    ) -> Result<CalendarEvent> {
        if new_id.is_empty() || new_id.contains(calendar_core::model::OCCURRENCE_SEPARATOR) {
            return Err(StorageError::Unsupported("invalid event id"));
        }
        let mut tx = self.pool.begin().await?;
        let master = Self::master_of(&mut tx, id).await?;
        if master.calendar_id == calendar_id {
            return Ok(master);
        }
        Self::writable_calendar(&mut tx, &master.calendar_id).await?;
        Self::writable_calendar(&mut tx, calendar_id).await?;
        let overrides = Self::overrides_of(&mut tx, &master.id).await?;

        // What the new calendar's provider can hold (Outlook: one reminder,
        // fewer repeat rules).
        for event in std::iter::once(&master).chain(&overrides) {
            self.check_reminders(&mut tx, calendar_id, Some(&event.reminders))
                .await?;
        }
        if let Some(rec) = &master.recurrence {
            self.check_series_supported(&mut tx, calendar_id, &master.timing, rec)
                .await?;
        }

        // The originals go the usual way: this queues the server deletion,
        // or forgets an object that was never sent.
        let href = Self::event_href(&mut tx, &master.id).await?;
        Self::delete_row(&mut tx, &master.id).await?; // overrides cascade
        if let Some(href) = href {
            Self::object_changed_or_removed(&mut tx, &master.calendar_id, &href).await?;
        }

        let at = now();
        let copy = |event: &CalendarEvent, id: String, series: Option<SeriesLink>| CalendarEvent {
            id,
            calendar_id: calendar_id.to_owned(),
            series,
            sync_status: SyncStatus::Saved,
            revision: event.revision + 1,
            updated_at: at,
            ..event.clone()
        };
        let mut moved = copy(&master, new_id.to_owned(), None);
        Self::insert(&mut tx, &moved).await?;
        for event in &overrides {
            let Some(link) = &event.series else { continue };
            let series = SeriesLink {
                series_id: new_id.to_owned(),
                recurrence_id: link.recurrence_id,
            };
            let id = occurrence_id(new_id, &link.recurrence_id);
            Self::insert(&mut tx, &copy(event, id, Some(series))).await?;
        }
        if let Some(url) = Self::calendar_remote(&mut tx, calendar_id).await? {
            Self::attach_new_object(&mut tx, calendar_id, &url, new_id).await?;
            moved.sync_status = SyncStatus::PendingSync;
        }
        tx.commit().await?;
        Ok(moved)
    }

    /// The stored row an id stands for: the event itself, or for an
    /// occurrence or override, its series master.
    async fn master_of(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<CalendarEvent> {
        let id = parse_occurrence_id(id).map_or(id, |(series, _)| series);
        let event = Self::load(tx, id).await?;
        match &event.series {
            Some(link) => Self::load(tx, &link.series_id).await,
            None => Ok(event),
        }
    }

    async fn overrides_of(
        tx: &mut Transaction<'_, Sqlite>,
        series_id: &str,
    ) -> Result<Vec<CalendarEvent>> {
        let rows: Vec<EventRow> = sqlx::query_as(concat!(
            "SELECT ",
            event_columns!(),
            " FROM events WHERE series_id = ?"
        ))
        .bind(series_id)
        .fetch_all(&mut **tx)
        .await?;
        rows.into_iter().map(decode_event).collect()
    }
}
