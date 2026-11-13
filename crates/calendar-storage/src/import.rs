//! Importing events from an iCalendar file into one calendar.
//!
//! Callers pass parsed objects (one per UID). Each becomes a new local
//! event, with its overrides, exactly as if it had been created in the
//! editor: in a synchronized calendar it is queued for upload. The whole
//! import is one transaction.
//!
//! Row ids derive from the calendar and the object's UID, so importing the
//! same file again skips what is already there instead of duplicating it.

use calendar_core::model::{MAX_REMINDER_MINUTES, MAX_REMINDERS};
use calendar_core::recurrence;
use calendar_core::{
    CalendarEvent, EventContent, ObjectContent, SeriesLink, SyncStatus, occurrence_id,
};
use serde::Serialize;
use sqlx::{Sqlite, Transaction};

use crate::{Result, StorageError, Store, now};

/// Most problems listed individually in a report.
const MAX_LISTED: usize = 50;

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Events imported (series count once).
    pub imported: usize,
    /// Events already in the calendar (same UID), left as they are.
    pub already_present: usize,
    /// Events that could not be imported, with the reason.
    pub failed: Vec<ImportProblem>,
    /// Imported events that had to be changed to fit the calendar.
    pub adjusted: Vec<ImportProblem>,
    /// More problems than listed in `failed` and `adjusted`.
    pub unlisted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportProblem {
    pub title: String,
    pub reason: String,
}

impl ImportReport {
    /// Records a skipped (`failed`) or changed event, listing at most
    /// [`MAX_LISTED`] problems and counting the rest.
    pub fn note(&mut self, failed: bool, title: &str, reason: String) {
        if self.failed.len() + self.adjusted.len() >= MAX_LISTED {
            self.unlisted += 1;
            return;
        }
        let title = if title.trim().is_empty() {
            "(No title)".to_owned()
        } else {
            title.to_owned()
        };
        let list = if failed {
            &mut self.failed
        } else {
            &mut self.adjusted
        };
        list.push(ImportProblem { title, reason });
    }
}

/// FNV-1a (64-bit), for stable ids.
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Row id of an imported object: the same file imported into the same
/// calendar gives the same ids.
fn import_id(calendar_id: &str, uid: &str) -> String {
    format!("import-{:016x}", fnv1a(&format!("{calendar_id}\n{uid}")))
}

/// Overrides without their series (e.g. one occurrence of a meeting sent
/// on its own) become one-off events.
fn standalone(object: ObjectContent) -> Vec<ObjectContent> {
    if object.master.is_some() {
        return vec![object];
    }
    object
        .overrides
        .into_iter()
        .map(|o| {
            let key = o.recurrence_id.unwrap_or_else(|| o.timing.start_key());
            ObjectContent {
                uid: format!("{}@{key}", object.uid),
                master: Some(EventContent {
                    recurrence_id: None,
                    ..o
                }),
                overrides: Vec::new(),
            }
        })
        .collect()
}

/// Reminders this calendar can hold, and whether some were dropped.
fn fit_reminders(reminders: &[u32], limit: Option<usize>) -> (Vec<u32>, bool) {
    let max = limit.unwrap_or(MAX_REMINDERS).min(MAX_REMINDERS);
    let kept: Vec<u32> = reminders
        .iter()
        .copied()
        .filter(|m| *m <= MAX_REMINDER_MINUTES)
        .take(max)
        .collect();
    let dropped = kept.len() < reminders.len();
    (kept, dropped)
}

fn check_component(c: &EventContent) -> std::result::Result<(), String> {
    if c.title.chars().count() > calendar_core::timing::MAX_TITLE_CHARS {
        return Err("the title is too long".into());
    }
    c.timing.validate().map_err(|e| e.to_string())?;
    if let Some(rec) = &c.recurrence {
        recurrence::validate(&c.timing, rec).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Store {
    /// Imports `objects` into `calendar_id` (which must be writable) in one
    /// transaction. Objects whose UID is already in the calendar are
    /// skipped; objects that are invalid or that the calendar's provider
    /// cannot hold are reported and skipped. Reminders beyond what the
    /// calendar holds are dropped (and reported).
    pub async fn import_objects(
        &self,
        calendar_id: &str,
        objects: Vec<ObjectContent>,
    ) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        let mut tx = self.pool.begin().await?;
        Self::writable_calendar(&mut tx, calendar_id).await?;
        let remote = Self::calendar_remote(&mut tx, calendar_id).await?;
        let limit = self.reminder_limit(&mut tx, calendar_id).await?;
        for object in objects.into_iter().flat_map(standalone) {
            let Some(master) = &object.master else {
                continue;
            };
            let id = import_id(calendar_id, &object.uid);
            if Self::already_present(&mut tx, calendar_id, &id, &object.uid).await? {
                report.already_present += 1;
                continue;
            }
            let checked = std::iter::once(master)
                .chain(&object.overrides)
                .try_for_each(check_component);
            if let Err(reason) = checked {
                report.note(true, &master.title, reason);
                continue;
            }
            if let Some(rec) = &master.recurrence {
                match self
                    .check_series_supported(&mut tx, calendar_id, &master.timing, rec)
                    .await
                {
                    Ok(()) => {}
                    Err(StorageError::UnsupportedRecurrence(reason)) => {
                        report.note(true, &master.title, reason);
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            let mut dropped = false;
            let mut row = |id: String, c: &EventContent, series: Option<SeriesLink>| {
                let (reminders, fewer) = fit_reminders(&c.reminders, limit);
                dropped |= fewer;
                let now = now();
                CalendarEvent {
                    id,
                    calendar_id: calendar_id.to_owned(),
                    title: c.title.clone(),
                    description: c.description.clone(),
                    location: c.location.clone(),
                    timing: c.timing.clone(),
                    recurrence: if series.is_none() {
                        Self::meaningful(c.recurrence.clone())
                    } else {
                        None
                    },
                    series,
                    reminders,
                    sync_status: SyncStatus::Saved,
                    revision: 1,
                    created_at: now,
                    updated_at: now,
                }
            };
            let mut rows = vec![row(id.clone(), master, None)];
            if rows[0].recurrence.is_some() {
                for o in &object.overrides {
                    let key = o.recurrence_id.unwrap_or_else(|| o.timing.start_key());
                    let link = SeriesLink {
                        series_id: id.clone(),
                        recurrence_id: key,
                    };
                    rows.push(row(occurrence_id(&id, &key), o, Some(link)));
                }
            }
            for event in &rows {
                Self::insert(&mut tx, event).await?;
            }
            if let Some(url) = &remote {
                Self::attach_new_object(&mut tx, calendar_id, url, &id).await?;
            }
            if dropped {
                let reason = match limit {
                    Some(1) => "kept only the first reminder (this calendar holds one)".to_owned(),
                    _ => "some reminders were left out".to_owned(),
                };
                report.note(false, &master.title, reason);
            }
            report.imported += 1;
        }
        tx.commit().await?;
        Ok(report)
    }

    /// Whether an object with this UID is already in the calendar: imported
    /// before, or downloaded from the server.
    async fn already_present(
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
        id: &str,
        uid: &str,
    ) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM events WHERE id = ?) + \
             (SELECT COUNT(*) FROM remote_objects WHERE calendar_id = ? AND uid = ?)",
        )
        .bind(id)
        .bind(calendar_id)
        .bind(uid)
        .fetch_one(&mut **tx)
        .await?;
        Ok(n > 0)
    }

    /// How many reminders per event the calendar's provider holds, if limited.
    async fn reminder_limit(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        calendar_id: &str,
    ) -> Result<Option<usize>> {
        let Some(kind) = Self::provider_of(tx, calendar_id).await? else {
            return Ok(None);
        };
        Ok(self
            .reminder_limits
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&kind)
            .copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::EventTiming;

    #[test]
    fn ids_are_stable_per_calendar() {
        assert_eq!(import_id("a", "x"), import_id("a", "x"));
        assert_ne!(import_id("a", "x"), import_id("b", "x"));
        assert!(!import_id("a", "x").contains(calendar_core::model::OCCURRENCE_SEPARATOR));
    }

    #[test]
    fn reminders_fit_the_calendar() {
        assert_eq!(fit_reminders(&[0, 15, 60], None), (vec![0, 15, 60], false));
        assert_eq!(fit_reminders(&[0, 15, 60], Some(1)), (vec![0], true));
        assert_eq!(
            fit_reminders(&[1, 2, 3, 4, 5, 6], None),
            (vec![1, 2, 3, 4, 5], true)
        );
        assert_eq!(
            fit_reminders(&[MAX_REMINDER_MINUTES + 1], None),
            (vec![], true)
        );
    }

    #[test]
    fn lone_overrides_become_events() {
        let timing = EventTiming::AllDay {
            start_date: chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(),
            end_date: chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap(),
        };
        let o = EventContent {
            title: "One occurrence".into(),
            description: String::new(),
            location: String::new(),
            recurrence_id: Some(timing.start_key()),
            timing,
            recurrence: None,
            reminders: vec![],
        };
        let split = standalone(ObjectContent {
            uid: "u".into(),
            master: None,
            overrides: vec![o],
        });
        assert_eq!(split.len(), 1);
        assert_eq!(split[0].uid, "u@20261120");
        assert!(split[0].master.as_ref().unwrap().recurrence_id.is_none());
    }
}
