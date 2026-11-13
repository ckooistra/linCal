//! Importing an iCalendar (.ics) file into a calendar.
//!
//! The file is split into one object per UID (as an ICS subscription is),
//! each read with the iCalendar codec, and the result is stored in one
//! transaction by `calendar-storage`. Parsing runs on the blocking pool:
//! files can be large and their content is untrusted.

use calendar_caldav::ICalendarCodec;
use calendar_core::ObjectContent;
use calendar_provider_api::ObjectCodec;
use calendar_storage::{ImportReport, StorageError, Store};

/// Largest file accepted.
pub const MAX_IMPORT_BYTES: usize = 20 * 1024 * 1024;

/// Why a file could not be imported at all.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0}")]
    File(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("a background task failed")]
    Task,
}

/// Objects read from a file, and the problems met while reading it.
struct Parsed {
    objects: Vec<ObjectContent>,
    /// (title, reason) of events that could not be read.
    unreadable: Vec<(String, String)>,
    /// (title, note) of events read only approximately.
    approximate: Vec<(String, String)>,
}

fn parse(text: &str) -> Result<Parsed, String> {
    let feed = calendar_ics::split_feed(text)
        .map_err(|_| "This file is not an iCalendar (.ics) file.".to_owned())?;
    let mut parsed = Parsed {
        objects: Vec::with_capacity(feed.objects.len()),
        unreadable: Vec::new(),
        approximate: Vec::new(),
    };
    for object in feed.objects {
        match ICalendarCodec.read(&object.data) {
            Ok(read) => {
                // Events without a UID got a stable one from their content.
                let content = read.content;
                if let Some(note) = read.warnings.into_iter().next() {
                    let title = content
                        .master
                        .iter()
                        .chain(&content.overrides)
                        .map(|e| e.title.clone())
                        .next()
                        .unwrap_or_default();
                    parsed.approximate.push((title, note));
                }
                parsed.objects.push(content);
            }
            Err(reason) => parsed.unreadable.push((String::new(), reason)),
        }
    }
    Ok(parsed)
}

/// Imports the events of `text` (the content of an .ics file) into
/// `calendar_id`. Fails only when nothing could be imported because of the
/// file or the calendar; problems with single events are in the report.
pub async fn import_ics(
    store: &Store,
    calendar_id: &str,
    text: String,
) -> Result<ImportReport, ImportError> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(ImportError::File(
            "This file is too large to import (the limit is 20 MB).".into(),
        ));
    }
    let parsed = tokio::task::spawn_blocking(move || parse(&text))
        .await
        .map_err(|_| ImportError::Task)?
        .map_err(ImportError::File)?;
    let mut report = store.import_objects(calendar_id, parsed.objects).await?;
    for (title, reason) in parsed.unreadable {
        report.note(true, &title, format!("could not be read: {reason}"));
    }
    for (title, note) in parsed.approximate {
        report.note(false, &title, note);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:series@test\r\nDTSTAMP:20261101T000000Z\r\n\
DTSTART;TZID=Europe/Berlin:20261102T090000\r\nDTEND;TZID=Europe/Berlin:20261102T093000\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Berlin:20261109T090000\r\nSUMMARY:Stand-up\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT10M\r\nDESCRIPTION:Stand-up\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:series@test\r\nDTSTAMP:20261101T000000Z\r\n\
RECURRENCE-ID;TZID=Europe/Berlin:20261116T090000\r\n\
DTSTART;TZID=Europe/Berlin:20261116T110000\r\nDTEND;TZID=Europe/Berlin:20261116T113000\r\n\
SUMMARY:Stand-up (late)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTAMP:20261101T000000Z\r\nDTSTART;VALUE=DATE:20261120\r\n\
DTEND;VALUE=DATE:20261121\r\nSUMMARY:No UID\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:task@test\r\nSUMMARY:A task\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_series_overrides_and_events_without_uid() {
        let parsed = parse(FILE).unwrap();
        assert!(parsed.unreadable.is_empty());
        assert_eq!(parsed.objects.len(), 2);
        let series = parsed
            .objects
            .iter()
            .find(|o| o.uid == "series@test")
            .unwrap();
        let master = series.master.as_ref().unwrap();
        assert_eq!(master.title, "Stand-up");
        assert_eq!(master.reminders, vec![10]);
        assert_eq!(master.recurrence.as_ref().unwrap().exdates.len(), 1);
        assert_eq!(series.overrides.len(), 1);
        let other = parsed
            .objects
            .iter()
            .find(|o| o.uid != "series@test")
            .unwrap();
        assert!(other.uid.starts_with("no-uid-"));
        // The same file gives the same id again (re-imports are detected).
        assert_eq!(parse(FILE).unwrap().objects.len(), 2);
        assert!(
            parse(FILE)
                .unwrap()
                .objects
                .iter()
                .any(|o| o.uid == other.uid)
        );
    }

    #[test]
    fn refuses_files_that_are_not_icalendar() {
        assert!(parse("hello").is_err());
        assert!(parse("BEGIN:VCARD\r\nEND:VCARD\r\n").is_err());
        let empty = parse("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n").unwrap();
        assert!(empty.objects.is_empty());
    }

    #[tokio::test]
    async fn imports_into_a_calendar() {
        let store = Store::open_in_memory().await.unwrap();
        store.ensure_local_calendar().await.unwrap();
        let calendar = store.list_calendars().await.unwrap().remove(0).id;
        let report = import_ics(&store, &calendar, FILE.to_owned())
            .await
            .unwrap();
        assert_eq!((report.imported, report.already_present), (2, 0));
        let again = import_ics(&store, &calendar, FILE.to_owned())
            .await
            .unwrap();
        assert_eq!((again.imported, again.already_present), (0, 2));
        assert!(matches!(
            import_ics(&store, &calendar, "not a calendar".into()).await,
            Err(ImportError::File(_))
        ));
    }
}
