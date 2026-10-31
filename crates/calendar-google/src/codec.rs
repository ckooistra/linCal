//! Google event JSON ⇄ the provider-neutral model.
//!
//! An object is a whole series as the provider stores it:
//! `{"timeZone": <calendar zone>, "events": [master, exceptions…]}` with
//! each event exactly as the API returned it.
//!
//! * Reading builds an equivalent VCALENDAR (Google expresses recurrence as
//!   RFC 5545 lines) and uses `calendar-ical`, so recurrence, exceptions and
//!   time zones are handled in one place.
//! * Writing produces a *plan*: patches of only the fields this app models
//!   and changed, one per series member, each conditional on that member's
//!   ETag. Attendees, reminders, conferencing and anything else stay as
//!   they are on the server.

use calendar_core::{EventContent, EventTiming, ObjectContent, OccurrenceKey, Recurrence};
use calendar_ical::content::escape_text;
use calendar_ical::values::{format_date, format_local, format_utc};
use calendar_provider_api::{ObjectCodec, ReadObject};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

/// Placeholder for the master event id in a plan, filled in by the provider
/// (new objects get their id from the href).
pub const MASTER: &str = "{master}";

pub struct GoogleCodec;

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The members of a stored object and the calendar's zone.
pub(crate) fn members(data: &str) -> Result<(Vec<Value>, Option<String>), String> {
    let v: Value = serde_json::from_str(data).map_err(|e| format!("not Google event data: {e}"))?;
    let events = v
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .ok_or("not Google event data: no events")?;
    Ok((events, str_field(&v, "timeZone").map(str::to_owned)))
}

pub(crate) fn is_cancelled(event: &Value) -> bool {
    str_field(event, "status") == Some("cancelled")
}

pub(crate) fn is_exception(event: &Value) -> bool {
    event.get("recurringEventId").is_some()
}

/// An iCalendar property for a Google `start`/`end`/`originalStartTime`.
fn time_line(name: &str, t: &Value, calendar_tz: Option<&str>) -> Result<String, String> {
    if let Some(date) = str_field(t, "date") {
        let d =
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| format!("bad date {date}"))?;
        return Ok(format!("{name};VALUE=DATE:{}", format_date(d)));
    }
    let raw = str_field(t, "dateTime").ok_or_else(|| format!("{name} without a time"))?;
    let instant = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| format!("bad time {raw}"))?
        .with_timezone(&Utc);
    let zone = str_field(t, "timeZone")
        .or(calendar_tz)
        .and_then(|z| z.parse::<Tz>().ok());
    Ok(match zone {
        Some(tz) if tz != chrono_tz::UTC => format!(
            "{name};TZID={}:{}",
            tz.name(),
            format_local(instant.with_timezone(&tz).naive_local())
        ),
        _ => format!("{name}:{}", format_utc(instant)),
    })
}

/// The VCALENDAR equivalent of a stored Google object.
fn to_icalendar(events: &[Value], calendar_tz: Option<&str>, uid: &str) -> Result<String, String> {
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n");
    for e in events {
        let cancelled = is_cancelled(e);
        out.push_str("BEGIN:VEVENT\r\n");
        out.push_str(&format!("UID:{uid}\r\n"));
        if let Some(original) = e.get("originalStartTime") {
            out.push_str(&time_line("RECURRENCE-ID", original, calendar_tz)?);
            out.push_str("\r\n");
        }
        // Cancelled exceptions may come without times: they only need to
        // say which occurrence is gone.
        let start = e.get("start").or(if cancelled {
            e.get("originalStartTime")
        } else {
            None
        });
        let start = start.ok_or("event without a start")?;
        out.push_str(&time_line("DTSTART", start, calendar_tz)?);
        out.push_str("\r\n");
        if let Some(end) = e.get("end") {
            out.push_str(&time_line("DTEND", end, calendar_tz)?);
            out.push_str("\r\n");
        }
        for (field, prop) in [
            ("summary", "SUMMARY"),
            ("description", "DESCRIPTION"),
            ("location", "LOCATION"),
        ] {
            if let Some(text) = str_field(e, field).filter(|t| !t.is_empty()) {
                out.push_str(&format!("{prop}:{}\r\n", escape_text(text)));
            }
        }
        if cancelled {
            out.push_str("STATUS:CANCELLED\r\n");
        }
        if !is_exception(e) {
            for line in e
                .get("recurrence")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                // RFC 5545 lines; refuse anything that could break framing.
                if line.contains(['\r', '\n']) {
                    return Err("malformed recurrence line".into());
                }
                out.push_str(line);
                out.push_str("\r\n");
            }
        }
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    Ok(out)
}

/// The occurrence key of an exception (`originalStartTime`).
fn original_key(e: &Value) -> Option<OccurrenceKey> {
    let t = e.get("originalStartTime")?;
    if let Some(date) = str_field(t, "date") {
        return NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .map(OccurrenceKey::Date);
    }
    let raw = str_field(t, "dateTime")?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|t| OccurrenceKey::Instant(t.with_timezone(&Utc)))
}

/// Google's id suffix for an occurrence: `<master>_<suffix>`.
pub fn instance_suffix(key: &OccurrenceKey) -> String {
    match key {
        OccurrenceKey::Instant(t) => format_utc(*t),
        OccurrenceKey::Floating(t) => format_utc(t.and_utc()),
        OccurrenceKey::Date(d) => format_date(*d),
    }
}

fn time_value(timing: &EventTiming, start: bool, calendar_tz: Option<&str>) -> Value {
    match timing {
        EventTiming::Zoned {
            start: s,
            end: e,
            time_zone,
        } => {
            let t = if start { s } else { e };
            json!({
                "dateTime": t.with_timezone(time_zone).to_rfc3339(),
                "timeZone": time_zone.name(),
                "date": Value::Null,
            })
        }
        // Google has no floating times: use the calendar's zone.
        EventTiming::Floating { start: s, end: e } => json!({
            "dateTime": (if start { s } else { e }).format("%Y-%m-%dT%H:%M:%S").to_string(),
            "timeZone": calendar_tz.unwrap_or("UTC"),
            "date": Value::Null,
        }),
        EventTiming::AllDay {
            start_date,
            end_date,
        } => json!({
            "date": (if start { start_date } else { end_date }).format("%Y-%m-%d").to_string(),
            "dateTime": Value::Null,
            "timeZone": Value::Null,
        }),
    }
}

fn key_line(name: &str, key: &OccurrenceKey, timing: &EventTiming) -> String {
    match (key, timing) {
        (OccurrenceKey::Instant(t), EventTiming::Zoned { time_zone, .. })
            if *time_zone != chrono_tz::UTC =>
        {
            format!(
                "{name};TZID={}:{}",
                time_zone.name(),
                format_local(t.with_timezone(time_zone).naive_local())
            )
        }
        (OccurrenceKey::Instant(t), _) => format!("{name}:{}", format_utc(*t)),
        (OccurrenceKey::Floating(t), _) => format!("{name}:{}", format_local(*t)),
        (OccurrenceKey::Date(d), _) => format!("{name};VALUE=DATE:{}", format_date(*d)),
    }
}

fn recurrence_lines(rec: Option<&Recurrence>, timing: &EventTiming) -> Value {
    let Some(rec) = rec else {
        return json!([]);
    };
    let mut lines = Vec::new();
    if !rec.rrule.trim().is_empty() {
        lines.push(format!("RRULE:{}", rec.rrule));
    }
    lines.extend(rec.exdates.iter().map(|k| key_line("EXDATE", k, timing)));
    lines.extend(rec.rdates.iter().map(|k| key_line("RDATE", k, timing)));
    json!(lines)
}

/// Fields of `desired` that differ from `base` (all of them without a base).
fn patch(
    desired: &EventContent,
    base: Option<&EventContent>,
    with_recurrence: bool,
    calendar_tz: Option<&str>,
) -> Map<String, Value> {
    let mut body = Map::new();
    for (field, new, old) in [
        ("summary", &desired.title, base.map(|b| &b.title)),
        (
            "description",
            &desired.description,
            base.map(|b| &b.description),
        ),
        ("location", &desired.location, base.map(|b| &b.location)),
    ] {
        if old != Some(new) {
            body.insert(field.into(), json!(new));
        }
    }
    if base.map(|b| &b.timing) != Some(&desired.timing) {
        body.insert(
            "start".into(),
            time_value(&desired.timing, true, calendar_tz),
        );
        body.insert(
            "end".into(),
            time_value(&desired.timing, false, calendar_tz),
        );
    }
    if with_recurrence && base.map(|b| &b.recurrence) != Some(&desired.recurrence) {
        body.insert(
            "recurrence".into(),
            recurrence_lines(desired.recurrence.as_ref(), &desired.timing),
        );
    }
    body
}

impl ObjectCodec for GoogleCodec {
    fn read(&self, data: &str) -> Result<ReadObject, String> {
        let (events, tz) = members(data)?;
        let uid = events
            .iter()
            .find_map(|e| str_field(e, "iCalUID"))
            .or_else(|| events.iter().find_map(|e| str_field(e, "id")))
            .ok_or("event without an id")?
            .to_owned();
        let text = to_icalendar(&events, tz.as_deref(), &uid)?;
        let cal = calendar_ical::parse(&text).map_err(|e| e.to_string())?;
        let mut read = calendar_ical::read_events(&cal).map_err(|e| e.to_string())?;
        let warnings = std::mem::take(&mut read.warnings);
        Ok(ReadObject {
            content: read.into_content(),
            warnings,
        })
    }

    fn write(
        &self,
        base: Option<&str>,
        uid: &str,
        content: &ObjectContent,
        renamed: &[(OccurrenceKey, OccurrenceKey)],
    ) -> String {
        let mut ops = Vec::new();
        let parsed = base.and_then(|b| Some((members(b).ok()?, self.read(b).ok()?.content)));
        match parsed {
            None => {
                // A new object: insert the master, then its overrides.
                if let Some(m) = &content.master {
                    ops.push(json!({
                        "method": "INSERT",
                        "eventId": MASTER,
                        "body": Value::Object(patch(m, None, true, None)),
                    }));
                }
                for o in &content.overrides {
                    if let Some(key) = &o.recurrence_id {
                        ops.push(json!({
                            "method": "PATCH",
                            "eventId": format!("{MASTER}_{}", instance_suffix(key)),
                            "body": Value::Object(patch(o, None, false, None)),
                        }));
                    }
                }
            }
            Some(((events, tz), base_content)) => {
                let tz = tz.as_deref();
                if let (Some(m), Some(bm)) = (&content.master, &base_content.master) {
                    let master_event = events.iter().find(|e| !is_exception(e));
                    let body = patch(m, Some(bm), true, tz);
                    if let Some(e) = master_event
                        && !body.is_empty()
                    {
                        ops.push(json!({
                            "method": "PATCH",
                            "eventId": str_field(e, "id"),
                            "etag": str_field(e, "etag"),
                            "body": Value::Object(body),
                        }));
                    }
                }
                for o in &content.overrides {
                    let Some(key) = o.recurrence_id else { continue };
                    let base_key = renamed
                        .iter()
                        .find(|(current, _)| *current == key)
                        .map_or(key, |(_, b)| *b);
                    let existing = events.iter().find(|e| {
                        is_exception(e) && !is_cancelled(e) && original_key(e) == Some(key)
                    });
                    let base_override = base_content
                        .overrides
                        .iter()
                        .find(|b| b.recurrence_id == Some(key));
                    let (event_id, etag, body) = match (existing, base_key == key) {
                        (Some(e), true) => (
                            str_field(e, "id").map(str::to_owned),
                            str_field(e, "etag"),
                            patch(o, base_override, false, tz),
                        ),
                        // No exception yet (or the series moved): patching
                        // the instance id creates one.
                        _ => (
                            Some(format!("{MASTER}_{}", instance_suffix(&key))),
                            None,
                            patch(o, None, false, tz),
                        ),
                    };
                    if !body.is_empty() {
                        ops.push(json!({
                            "method": "PATCH",
                            "eventId": event_id,
                            "etag": etag,
                            "body": Value::Object(body),
                        }));
                    }
                }
                // Exceptions the series no longer generates (it was cut
                // short, e.g. by "this and following") are removed rather
                // than left orphaned.
                if let Some(m) = &content.master {
                    for e in events
                        .iter()
                        .filter(|e| is_exception(e) && !is_cancelled(e))
                    {
                        let Some(key) = original_key(e) else { continue };
                        let kept = content.overrides.iter().any(|o| {
                            o.recurrence_id.is_some_and(|k| {
                                k == key || renamed.iter().any(|(c, b)| *c == k && *b == key)
                            })
                        });
                        let generated = match &m.recurrence {
                            Some(rec) => calendar_core::recurrence::has_occurrence(
                                &m.timing,
                                rec,
                                &key,
                                &chrono_tz::UTC,
                            )
                            .unwrap_or(true),
                            None => false,
                        };
                        if !kept && !generated {
                            ops.push(json!({
                                "method": "DELETE",
                                "eventId": str_field(e, "id"),
                                "etag": str_field(e, "etag"),
                            }));
                        }
                    }
                }
            }
        }
        json!({ "uid": uid, "ops": ops }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calendar_core::timing::resolve_local;

    const SERIES: &str = r#"{"timeZone":"Europe/Berlin","events":[
      {"id":"abc","etag":"\"1\"","status":"confirmed","iCalUID":"abc@google.com","summary":"Stand-up",
       "start":{"dateTime":"2026-06-01T09:00:00+02:00","timeZone":"Europe/Berlin"},
       "end":{"dateTime":"2026-06-01T09:15:00+02:00","timeZone":"Europe/Berlin"},
       "recurrence":["RRULE:FREQ=DAILY;COUNT=5"],
       "attendees":[{"email":"bob@example.com"}],"conferenceData":{"x":1}},
      {"id":"abc_20260603T070000Z","etag":"\"2\"","status":"confirmed","iCalUID":"abc@google.com",
       "recurringEventId":"abc","originalStartTime":{"dateTime":"2026-06-03T09:00:00+02:00","timeZone":"Europe/Berlin"},
       "summary":"Stand-up (late)",
       "start":{"dateTime":"2026-06-03T11:00:00+02:00"},"end":{"dateTime":"2026-06-03T11:15:00+02:00"}},
      {"id":"abc_20260604T070000Z","etag":"\"3\"","status":"cancelled","recurringEventId":"abc",
       "originalStartTime":{"dateTime":"2026-06-04T09:00:00+02:00","timeZone":"Europe/Berlin"}}
    ]}"#;

    fn berlin(d: u32, h: u32) -> chrono::DateTime<Utc> {
        resolve_local(
            &chrono_tz::Europe::Berlin,
            NaiveDate::from_ymd_opt(2026, 6, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
        )
    }

    #[test]
    fn reads_series_exceptions_and_cancellations() {
        let read = GoogleCodec.read(SERIES).unwrap();
        let c = read.content;
        assert_eq!(c.uid, "abc@google.com");
        let m = c.master.unwrap();
        assert_eq!(m.title, "Stand-up");
        assert!(
            matches!(m.timing, EventTiming::Zoned { time_zone, .. } if time_zone == chrono_tz::Europe::Berlin)
        );
        let rec = m.recurrence.unwrap();
        assert_eq!(rec.rrule, "FREQ=DAILY;COUNT=5");
        assert_eq!(rec.exdates, vec![OccurrenceKey::Instant(berlin(4, 9))]);
        assert_eq!(c.overrides.len(), 1);
        assert_eq!(
            c.overrides[0].recurrence_id,
            Some(OccurrenceKey::Instant(berlin(3, 9)))
        );
        assert_eq!(c.overrides[0].title, "Stand-up (late)");
    }

    fn plan(json: &str) -> Vec<Value> {
        serde_json::from_str::<Value>(json).unwrap()["ops"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn writes_only_changed_fields_with_member_etags() {
        let base = GoogleCodec.read(SERIES).unwrap().content;
        let mut edited = base.clone();
        edited.master.as_mut().unwrap().title = "Daily".into();
        edited.overrides[0].location = "Room 2".into();
        let ops = plan(&GoogleCodec.write(Some(SERIES), "abc@google.com", &edited, &[]));
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0]["eventId"], "abc");
        assert_eq!(ops[0]["etag"], "\"1\"");
        assert_eq!(ops[0]["body"], json!({"summary": "Daily"}));
        assert_eq!(ops[1]["eventId"], "abc_20260603T070000Z");
        assert_eq!(ops[1]["etag"], "\"2\"");
        assert_eq!(ops[1]["body"], json!({"location": "Room 2"}));
        // Nothing changed: nothing to send.
        assert!(plan(&GoogleCodec.write(Some(SERIES), "abc@google.com", &base, &[])).is_empty());
    }

    #[test]
    fn new_overrides_and_cancellations() {
        let base = GoogleCodec.read(SERIES).unwrap().content;
        let mut edited = base.clone();
        let mut moved = edited.overrides[0].clone();
        moved.recurrence_id = Some(OccurrenceKey::Instant(berlin(5, 9)));
        moved.title = "Friday".into();
        edited.overrides.push(moved);
        edited
            .master
            .as_mut()
            .unwrap()
            .recurrence
            .as_mut()
            .unwrap()
            .exdates
            .push(OccurrenceKey::Instant(berlin(2, 9)));
        let ops = plan(&GoogleCodec.write(Some(SERIES), "abc@google.com", &edited, &[]));
        let master = ops.iter().find(|o| o["eventId"] == "abc").unwrap();
        let lines = master["body"]["recurrence"].as_array().unwrap();
        assert!(lines.contains(&json!("EXDATE;TZID=Europe/Berlin:20260602T090000")));
        let created = ops
            .iter()
            .find(|o| o["eventId"] == "{master}_20260605T070000Z")
            .unwrap();
        assert_eq!(created["body"]["summary"], "Friday");
        assert!(created["etag"].is_null());
    }

    #[test]
    fn new_objects_are_inserted() {
        let base = GoogleCodec.read(SERIES).unwrap().content;
        let ops = plan(&GoogleCodec.write(None, "local-1", &base, &[]));
        assert_eq!(ops[0]["method"], "INSERT");
        assert_eq!(ops[0]["body"]["summary"], "Stand-up");
        assert_eq!(ops[0]["body"]["recurrence"][0], "RRULE:FREQ=DAILY;COUNT=5");
        assert_eq!(ops[1]["eventId"], "{master}_20260603T070000Z");
    }

    #[test]
    fn all_day_and_malformed_data() {
        let data = r#"{"events":[{"id":"d","iCalUID":"d","summary":"Holiday","start":{"date":"2026-12-25"},"end":{"date":"2026-12-26"}}]}"#;
        let c = GoogleCodec.read(data).unwrap().content;
        assert!(matches!(
            c.master.unwrap().timing,
            EventTiming::AllDay { .. }
        ));
        assert!(GoogleCodec.read("{}").is_err());
        let injected = r#"{"events":[{"id":"x","start":{"date":"2026-12-25"},"recurrence":["RRULE:FREQ=DAILY\r\nEND:VEVENT"]}]}"#;
        assert!(GoogleCodec.read(injected).is_err());
    }
}
