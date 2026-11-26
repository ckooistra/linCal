//! Moving an event (or a whole series) to another calendar.
//!
//! **Within an account whose provider can move objects** (CalDAV `MOVE`,
//! Google `events.move`, registered with [`Store::set_moves_on_server`]),
//! the rows and their remote object change calendar at once, keeping their
//! ids, and a `move` operation is queued. The server keeps the event's
//! identity: its UID, attendees, conferencing, everything the application
//! doesn't model. Until the move is sent, the queue entry names the
//! calendar the server still has it in (`move_from`), so syncing that
//! calendar doesn't import it again. If the server can't move it after all,
//! [`Store::copy_instead_of_move`] turns it into a copy.
//!
//! **Otherwise** (between accounts, to or from a local calendar, Outlook),
//! a move is a deletion plus a creation: the rows are copied into the new
//! calendar under a new id, and the originals are deleted the usual way.
//! In one transaction:
//!
//! * the copy, in a synchronized calendar, is queued for upload as a new
//!   object;
//! * the old server copy, if there is one, is queued for deletion (with
//!   its version, so a change made there meanwhile becomes a conflict
//!   instead of being deleted). The deletion waits until the copy has
//!   been uploaded: while the new server refuses the copy, the original
//!   stays on its server. If the user gives the copy up ("Discard my
//!   change", or removes its account), the deletion is dropped and the
//!   original comes back with the next sync.
//!
//! The local rows are the source of truth throughout, so a failed upload
//! or deletion is retried and nothing is lost. The new id keeps the copy
//! apart from the old server copy, should that come back.
//!
//! What the app doesn't model (attendees, conferencing) is not copied, as
//! with "this and following" splits: the server sees a new event.

use calendar_core::{
    CalendarEvent, EventDraft, ProviderKind, SeriesLink, SyncStatus, occurrence_id,
    parse_occurrence_id,
};
use sqlx::{Sqlite, Transaction};

use crate::codec::{EventRow, decode_event, event_columns};
use crate::remote::PendingOp;
use crate::{Result, StorageError, Store, now};

/// A new id for the copy made when the server can't move an event.
fn copy_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!(
        "local-moved-{nanos:x}-{:x}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

impl Store {
    /// Says that `provider`'s servers can move an event between calendars
    /// of an account (see the module documentation).
    pub fn set_moves_on_server(&self, provider: ProviderKind) {
        self.moves_on_server
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(provider);
    }

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
    /// as a whole, with its modified and cancelled occurrences. A move the
    /// server makes keeps the event's id; a copy gets the id `new_id`. The
    /// moved event is returned as stored now.
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

        let href = Self::event_href(&mut tx, &master.id).await?;
        if let Some(href) = &href
            && self
                .moves_on_server(&mut tx, &master, href, calendar_id)
                .await?
        {
            let moved = Self::relocate(&mut tx, &master, href, calendar_id).await?;
            tx.commit().await?;
            return Ok(moved);
        }
        let moved = Self::copy_to(&mut tx, &master, &overrides, href, new_id, calendar_id).await?;
        tx.commit().await?;
        Ok(moved)
    }

    /// When the server can't move an object after all (another organizer's
    /// invitation on Google, a CalDAV server without cross-collection
    /// `MOVE`): puts it back where the server has it, and copies it to the
    /// calendar it was moved to, as a move between accounts does. Returns
    /// the operations now queued (the deletion and the upload), or nothing
    /// if the event changed meanwhile (the next pass tries again).
    pub async fn copy_instead_of_move(&self, op: &PendingOp) -> Result<Vec<PendingOp>> {
        let mut tx = self.pool.begin().await?;
        let current: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT generation, move_from FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(&op.calendar_id)
        .bind(&op.href)
        .fetch_optional(&mut *tx)
        .await?;
        let origin = match current {
            Some((generation, Some(origin))) if generation == op.generation => origin,
            _ => return Ok(Vec::new()),
        };
        for sql in [
            "DELETE FROM pending_operations WHERE calendar_id = ?2 AND href = ?3",
            "UPDATE remote_objects SET calendar_id = ?1 WHERE calendar_id = ?2 AND href = ?3",
            "UPDATE events SET calendar_id = ?1 WHERE calendar_id = ?2 AND remote_href = ?3",
        ] {
            sqlx::query(sql)
                .bind(&origin)
                .bind(&op.calendar_id)
                .bind(&op.href)
                .execute(&mut *tx)
                .await?;
        }
        let master_id: String = sqlx::query_scalar(
            "SELECT id FROM events WHERE calendar_id = ? AND remote_href = ? AND series_id IS NULL",
        )
        .bind(&origin)
        .bind(&op.href)
        .fetch_one(&mut *tx)
        .await?;
        let master = Self::load(&mut tx, &master_id).await?;
        let overrides = Self::overrides_of(&mut tx, &master_id).await?;
        let new_id = copy_id();
        Self::copy_to(
            &mut tx,
            &master,
            &overrides,
            Some(op.href.clone()),
            &new_id,
            &op.calendar_id,
        )
        .await?;
        let new_href = Self::event_href(&mut tx, &new_id).await?;
        tx.commit().await?;
        let mut queued = Vec::new();
        for (calendar, href) in [
            (origin, Some(op.href.clone())),
            (op.calendar_id.clone(), new_href),
        ] {
            if let Some(href) = href
                && let Some(next) = self.pending_operation(&calendar, &href).await?
            {
                queued.push(next);
            }
        }
        Ok(queued)
    }

    /// Whether the server will move `master`'s object (`href`) to
    /// `calendar_id`: another calendar of the same account, with a provider
    /// that can, for an object the server has and that holds only this
    /// event (not lone occurrences of another series).
    async fn moves_on_server(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        master: &CalendarEvent,
        href: &str,
        calendar_id: &str,
    ) -> Result<bool> {
        let row: Option<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
            "SELECT a.provider, f.remote_url, f.account_id, t.remote_url FROM calendars f \
             JOIN accounts a ON a.id = f.account_id \
             JOIN calendars t ON t.id = ? AND t.account_id = f.account_id WHERE f.id = ?",
        )
        .bind(calendar_id)
        .bind(&master.calendar_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((provider, Some(_), _, Some(_))) = row else {
            return Ok(false);
        };
        let supported = ProviderKind::parse(&provider).is_some_and(|kind| {
            self.moves_on_server
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&kind)
        });
        if !supported {
            return Ok(false);
        }
        let presence: Option<String> = sqlx::query_scalar(
            "SELECT presence FROM remote_objects WHERE calendar_id = ? AND href = ?",
        )
        .bind(&master.calendar_id)
        .bind(href)
        .fetch_optional(&mut **tx)
        .await?;
        let others: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE calendar_id = ? AND remote_href = ? \
             AND id <> ? AND series_id IS NOT ?",
        )
        .bind(&master.calendar_id)
        .bind(href)
        .bind(&master.id)
        .bind(&master.id)
        .fetch_one(&mut **tx)
        .await?;
        Ok(presence.as_deref() == Some("present") && others == 0)
    }

    /// The move within an account: rows and remote object change calendar,
    /// and the queue says where the server has it.
    async fn relocate(
        tx: &mut Transaction<'_, Sqlite>,
        master: &CalendarEvent,
        href: &str,
        to: &str,
    ) -> Result<CalendarEvent> {
        let from = master.calendar_id.as_str();
        let queued: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT kind, move_from FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(from)
        .bind(href)
        .fetch_optional(&mut **tx)
        .await?;
        for sql in [
            "UPDATE remote_objects SET calendar_id = ?1 WHERE calendar_id = ?2 AND href = ?3",
            "UPDATE events SET calendar_id = ?1, revision = revision + 1, updated_at = ?4 \
             WHERE calendar_id = ?2 AND remote_href = ?3",
        ] {
            sqlx::query(sql)
                .bind(to)
                .bind(from)
                .bind(href)
                .bind(now().timestamp_millis())
                .execute(&mut **tx)
                .await?;
        }
        match queued {
            None => {
                sqlx::query(
                    "INSERT INTO pending_operations (calendar_id, href, kind, created_at, move_from) \
                     VALUES (?, ?, 'move', ?, ?)",
                )
                .bind(to)
                .bind(href)
                .bind(now().timestamp_millis())
                .bind(from)
                .execute(&mut **tx)
                .await?;
            }
            // Moved back to where the server has it: nothing to move.
            Some((kind, origin)) if origin.as_deref().unwrap_or(from) == to && kind == "move" => {
                sqlx::query("DELETE FROM pending_operations WHERE calendar_id = ? AND href = ?")
                    .bind(from)
                    .bind(href)
                    .execute(&mut **tx)
                    .await?;
            }
            Some((_, origin)) => {
                let origin = origin.unwrap_or_else(|| from.to_owned());
                sqlx::query(
                    "UPDATE pending_operations SET calendar_id = ?, move_from = ?, \
                     generation = generation + 1 WHERE calendar_id = ? AND href = ?",
                )
                .bind(to)
                .bind((origin != to).then_some(origin))
                .bind(from)
                .bind(href)
                .execute(&mut **tx)
                .await?;
            }
        }
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM pending_operations WHERE calendar_id = ? AND href = ?",
        )
        .bind(to)
        .bind(href)
        .fetch_optional(&mut **tx)
        .await?;
        let status = match state.as_deref() {
            None => SyncStatus::Synced,
            Some("conflict") => SyncStatus::Conflict,
            Some("failed") => SyncStatus::SyncFailed,
            Some(_) => SyncStatus::PendingSync,
        };
        sqlx::query("UPDATE events SET sync_status = ? WHERE calendar_id = ? AND remote_href = ?")
            .bind(status.as_str())
            .bind(to)
            .bind(href)
            .execute(&mut **tx)
            .await?;
        Self::load(tx, &master.id).await
    }

    /// The move as a copy: the originals are deleted the usual way, and
    /// copies under `new_id` are created in `calendar_id`.
    async fn copy_to(
        tx: &mut Transaction<'_, Sqlite>,
        master: &CalendarEvent,
        overrides: &[CalendarEvent],
        href: Option<String>,
        new_id: &str,
        calendar_id: &str,
    ) -> Result<CalendarEvent> {
        // The originals go the usual way: this queues the server deletion,
        // or forgets an object that was never sent.
        Self::delete_row(tx, &master.id).await?; // overrides cascade
        if let Some(href) = &href {
            Self::object_changed_or_removed(tx, &master.calendar_id, href).await?;
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
        let mut moved = copy(master, new_id.to_owned(), None);
        Self::insert(tx, &moved).await?;
        for event in overrides {
            let Some(link) = &event.series else { continue };
            let series = SeriesLink {
                series_id: new_id.to_owned(),
                recurrence_id: link.recurrence_id,
            };
            let id = occurrence_id(new_id, &link.recurrence_id);
            Self::insert(tx, &copy(event, id, Some(series))).await?;
        }
        let copied = match Self::calendar_remote(tx, calendar_id).await? {
            Some(url) => {
                Self::attach_new_object(tx, calendar_id, &url, new_id).await?;
                moved.sync_status = SyncStatus::PendingSync;
                Self::event_href(tx, new_id).await?
            }
            None => None,
        };
        // The original is deleted on its server only once the copy is on
        // the new one, so a copy the server refuses loses nothing.
        if let Some(href) = &href {
            Self::delete_after_copy(
                tx,
                (&master.calendar_id, href),
                copied.as_deref().map(|h| (calendar_id, h)),
            )
            .await?;
        }
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
