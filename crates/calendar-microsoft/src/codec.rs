//! Microsoft Graph event JSON ⇄ the provider-neutral model.
//!
//! An object is either a single event, or a whole series: the series
//! master as read with `$expand=exceptionOccurrences` and
//! `cancelledOccurrences` selected, so the master's pattern, its
//! exceptions and its cancelled occurrences arrive together. Pattern ⇄
//! RRULE translation lives in [`crate::pattern`].
//!
//! Times are read in UTC (`Prefer: outlook.timezone="UTC"`); the event's
//! own zone (`originalStartTimeZone`, Windows or IANA name) is kept for
//! display and recurrence. Writes send local times in that zone, so series
//! follow its daylight-saving rules.
//!
//! Writing produces a *plan* of patches of only the fields this app models
//! and changed, so attendees, online-meeting data, categories and the rest
//! stay untouched. Graph cannot address an occurrence by its original
//! start directly, so plan steps for new exceptions and cancellations name
//! the occurrence key and the provider looks the instance up.

use calendar_core::{EventContent, EventTiming, ObjectContent, OccurrenceKey, Recurrence};
use calendar_provider_api::{ObjectCodec, ReadObject};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use crate::pattern;

/// Placeholder for the master event id in a plan, filled in by the provider
/// (a new series gets its id when it is created).
pub const MASTER: &str = "{master}";

pub struct GraphCodec;

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

pub(crate) fn is_series(e: &Value) -> bool {
    str_field(e, "type") == Some("seriesMaster")
}

/// Graph's `dateTime` (UTC because of the Prefer header): fractional
/// seconds with up to seven digits.
fn parse_utc(t: &Value) -> Result<NaiveDateTime, String> {
    let raw = str_field(t, "dateTime").ok_or("time without dateTime")?;
    let zone = str_field(t, "timeZone").unwrap_or("UTC");
    if !zone.eq_ignore_ascii_case("UTC") {
        return Err(format!("unexpected time zone {zone} (expected UTC)"));
    }
    let main = raw.split('.').next().unwrap_or(raw);
    NaiveDateTime::parse_from_str(main, "%Y-%m-%dT%H:%M:%S").map_err(|_| format!("bad time {raw}"))
}

/// Plain text of an HTML body (Graph bodies are often HTML).
fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match (c, in_tag) {
            ('<', false) => {
                in_tag = true;
                tag.clear();
            }
            ('>', true) => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "br" | "p" | "div" | "li"
                ) && !out.ends_with('\n')
                {
                    out.push('\n');
                }
            }
            (c, true) => tag.push(c),
            (c, false) => out.push(c),
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn description(e: &Value) -> String {
    let body = &e["body"];
    let content = str_field(body, "content").unwrap_or("");
    if str_field(body, "contentType").is_some_and(|t| t.eq_ignore_ascii_case("html")) {
        html_to_text(content)
    } else {
        content.trim().to_owned()
    }
}

/// The event's own zone, if Graph names one this app knows.
fn zone_of(e: &Value) -> Option<Tz> {
    let zone = str_field(e, "originalStartTimeZone")?;
    calendar_ical::values::resolve_tzid(zone, &calendar_ical::Component::new("VCALENDAR"))
}

/// One Graph event as provider-neutral content (without recurrence).
pub fn event_content(e: &Value) -> Result<(EventContent, Vec<String>), String> {
    let mut warnings = Vec::new();
    let start = parse_utc(&e["start"])?;
    let end = parse_utc(&e["end"])?;
    let timing = if e["isAllDay"].as_bool() == Some(true) {
        let s = start.date();
        let end = end.date().max(s + chrono::Duration::days(1));
        EventTiming::AllDay {
            start_date: s,
            end_date: end,
        }
    } else {
        let tz = zone_of(e).unwrap_or_else(|| {
            if let Some(zone) = str_field(e, "originalStartTimeZone") {
                warnings.push(format!("unknown time zone {zone:?}; shown in UTC"));
            }
            chrono_tz::UTC
        });
        let start = start.and_utc();
        EventTiming::Zoned {
            start,
            end: end.and_utc().max(start),
            time_zone: tz,
        }
    };
    Ok((
        EventContent {
            reminders: reminders(e),
            title: str_field(e, "subject").unwrap_or("").to_owned(),
            description: description(e),
            location: str_field(&e["location"], "displayName")
                .unwrap_or("")
                .to_owned(),
            timing,
            recurrence: None,
            recurrence_id: None,
        },
        warnings,
    ))
}

/// Outlook has at most one reminder per event.
fn reminders(e: &Value) -> Vec<u32> {
    if e.get("isReminderOn").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    e.get("reminderMinutesBeforeStart")
        .and_then(Value::as_u64)
        .and_then(|m| u32::try_from(m).ok())
        .filter(|m| *m <= calendar_core::MAX_REMINDER_MINUTES)
        .into_iter()
        .collect()
}

/// The key of the series slot that started at `t` (an `originalStart`).
fn key_at(t: DateTime<Utc>, master: &EventTiming, zone: Option<Tz>) -> OccurrenceKey {
    match master {
        EventTiming::Zoned { .. } => OccurrenceKey::Instant(t),
        EventTiming::Floating { .. } => OccurrenceKey::Floating(t.naive_utc()),
        // All-day starts may be given as midnight UTC or as midnight in the
        // event's zone: the nearest midnight in that zone names the day.
        EventTiming::AllDay { .. } => {
            let tz = zone.unwrap_or(chrono_tz::UTC);
            OccurrenceKey::Date(
                (t + chrono::Duration::hours(12))
                    .with_timezone(&tz)
                    .date_naive(),
            )
        }
    }
}

/// The key of the series slot on `date` (`cancelledOccurrences` name days).
fn key_on(date: NaiveDate, master: &EventTiming) -> OccurrenceKey {
    match master {
        EventTiming::Zoned {
            start, time_zone, ..
        } => {
            let time = start.with_timezone(time_zone).time();
            OccurrenceKey::Instant(calendar_core::timing::resolve_local(
                time_zone,
                date.and_time(time),
            ))
        }
        EventTiming::Floating { start, .. } => OccurrenceKey::Floating(date.and_time(start.time())),
        EventTiming::AllDay { .. } => OccurrenceKey::Date(date),
    }
}

/// The key of an occurrence or exception of `master` (from `originalStart`).
pub(crate) fn instance_key(instance: &Value, master: &Value) -> Option<OccurrenceKey> {
    let raw = str_field(instance, "originalStart")?;
    let t = DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc);
    let (content, _) = event_content(master).ok()?;
    Some(key_at(t, &content.timing, zone_of(master)))
}

/// Exceptions of a series as read (empty for single events).
pub(crate) fn exceptions(e: &Value) -> impl Iterator<Item = &Value> {
    e["exceptionOccurrences"].as_array().into_iter().flatten()
}

/// An event's ETag, rebuilt from its change key where Graph left it out
/// (expanded exceptions).
pub(crate) fn etag_of(e: &Value) -> Option<String> {
    str_field(e, "@odata.etag")
        .map(str::to_owned)
        .or_else(|| str_field(e, "changeKey").map(|k| format!("W/\"{k}\"")))
}

fn read_series(m: &Value) -> Result<(ObjectContent, Vec<String>), String> {
    let (mut master, mut warnings) = event_content(m)?;
    let zone = zone_of(m);
    let rrule = pattern::to_rrule(&m["recurrence"], &master.timing)?;
    let mut exdates: Vec<OccurrenceKey> = m["cancelledOccurrences"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        // "OID.<master id>.<yyyy-mm-dd>"
        .filter_map(|oid| oid.rsplit('.').next())
        .filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .map(|d| key_on(d, &master.timing))
        .collect();
    exdates.sort();
    exdates.dedup();
    let mut overrides = Vec::new();
    for x in exceptions(m) {
        let Some(t) = str_field(x, "originalStart")
            .and_then(|r| DateTime::parse_from_rfc3339(r).ok())
            .map(|t| t.with_timezone(&Utc))
        else {
            warnings.push("an exception without its original start was skipped".into());
            continue;
        };
        let (mut content, w) = event_content(x)?;
        warnings.extend(w);
        content.recurrence_id = Some(key_at(t, &master.timing, zone));
        overrides.push(content);
    }
    master.recurrence = Some(Recurrence {
        rrule,
        rdates: Vec::new(),
        exdates,
    });
    let uid = str_field(m, "iCalUId")
        .or(str_field(m, "id"))
        .ok_or("event without an id")?
        .to_owned();
    Ok((
        ObjectContent {
            uid,
            master: Some(master),
            overrides,
        },
        warnings,
    ))
}

fn pick<T>(start: bool, s: T, e: T) -> T {
    if start { s } else { e }
}

fn time_value(timing: &EventTiming, start: bool) -> (Value, bool) {
    match timing {
        EventTiming::Zoned {
            start: s,
            end: e,
            time_zone,
        } => (
            json!({
                "dateTime": pick(start, s, e).with_timezone(time_zone).format("%Y-%m-%dT%H:%M:%S").to_string(),
                "timeZone": pattern::zone_name(*time_zone),
            }),
            false,
        ),
        // Graph has no floating times.
        EventTiming::Floating { start: s, end: e } => (
            json!({"dateTime": pick(start, s, e).format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"}),
            false,
        ),
        EventTiming::AllDay {
            start_date,
            end_date,
        } => {
            let d = pick(start, start_date, end_date);
            (
                json!({"dateTime": format!("{}T00:00:00", d.format("%Y-%m-%d")), "timeZone": "UTC"}),
                true,
            )
        }
    }
}

/// Changed fields of `desired` relative to `base` (all without a base).
pub fn patch(desired: &EventContent, base: Option<&EventContent>) -> Map<String, Value> {
    let mut body = Map::new();
    if base.map(|b| &b.title) != Some(&desired.title) {
        body.insert("subject".into(), json!(desired.title));
    }
    if base.map(|b| &b.description) != Some(&desired.description) {
        body.insert(
            "body".into(),
            json!({"contentType": "text", "content": desired.description}),
        );
    }
    if base.map(|b| &b.location) != Some(&desired.location) {
        body.insert("location".into(), json!({"displayName": desired.location}));
    }
    // Outlook keeps one reminder: the earliest one is sent. A new event
    // without reminders keeps the mailbox default.
    if base.map(|b| &b.reminders) != Some(&desired.reminders)
        && (base.is_some() || !desired.reminders.is_empty())
    {
        match desired.reminders.iter().max() {
            Some(m) => {
                body.insert("isReminderOn".into(), json!(true));
                body.insert("reminderMinutesBeforeStart".into(), json!(m));
            }
            None => {
                body.insert("isReminderOn".into(), json!(false));
            }
        }
    }
    let timing_changed = base.map(|b| &b.timing) != Some(&desired.timing);
    if timing_changed {
        let (start, all_day) = time_value(&desired.timing, true);
        body.insert("start".into(), start);
        body.insert("end".into(), time_value(&desired.timing, false).0);
        body.insert("isAllDay".into(), json!(all_day));
    }
    fn rule(c: Option<&EventContent>) -> Option<&str> {
        c.and_then(|c| c.recurrence.as_ref())
            .map(|r| r.rrule.as_str())
    }
    if rule(base) != rule(Some(desired)) || (timing_changed && desired.recurrence.is_some()) {
        // The pattern's start date follows the series start.
        let value = match &desired.recurrence {
            Some(rec) => pattern::to_graph(&rec.rrule, &desired.timing).unwrap_or(Value::Null),
            None => Value::Null,
        };
        if !(value.is_null() && base.is_none()) {
            body.insert("recurrence".into(), value);
        }
    }
    body
}

fn key_json(key: &OccurrenceKey) -> Value {
    match key {
        OccurrenceKey::Instant(t) => json!({"instant": t.to_rfc3339()}),
        OccurrenceKey::Floating(t) => {
            json!({"floating": t.format("%Y-%m-%dT%H:%M:%S").to_string()})
        }
        OccurrenceKey::Date(d) => json!({"date": d.format("%Y-%m-%d").to_string()}),
    }
}

/// The occurrence key named by a plan step.
pub(crate) fn plan_key(v: &Value) -> Option<OccurrenceKey> {
    if let Some(t) = str_field(v, "instant") {
        return DateTime::parse_from_rfc3339(t)
            .ok()
            .map(|t| OccurrenceKey::Instant(t.with_timezone(&Utc)));
    }
    if let Some(t) = str_field(v, "floating") {
        return NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .map(OccurrenceKey::Floating);
    }
    NaiveDate::parse_from_str(str_field(v, "date")?, "%Y-%m-%d")
        .ok()
        .map(OccurrenceKey::Date)
}

impl ObjectCodec for GraphCodec {
    fn read(&self, data: &str) -> Result<ReadObject, String> {
        let e: Value =
            serde_json::from_str(data).map_err(|err| format!("not Graph event data: {err}"))?;
        let (content, warnings) = if is_series(&e) {
            read_series(&e)?
        } else {
            let uid = str_field(&e, "iCalUId")
                .or(str_field(&e, "id"))
                .ok_or("event without an id")?
                .to_owned();
            let (content, mut warnings) = event_content(&e)?;
            if str_field(&e, "type").is_some_and(|t| t == "occurrence" || t == "exception") {
                // An instance whose series could not be read with it.
                warnings.push("occurrence of a Microsoft series read on its own".into());
            }
            (
                ObjectContent {
                    uid,
                    master: Some(content),
                    overrides: Vec::new(),
                },
                warnings,
            )
        };
        Ok(ReadObject { content, warnings })
    }

    /// `{"uid": …, "ops": [step…]}` with steps
    /// * `{"op": "create", "body"}`: a new event (or series);
    /// * `{"op": "patch", "id", "etag", "body"}`: the master or an exception;
    /// * `{"op": "patchInstance", "key", "body"}`: an occurrence that is not
    ///   an exception yet (it becomes one);
    /// * `{"op": "cancelInstance", "key"}`: an occurrence to delete;
    /// * `{"op": "delete", "id"}`: an exception the series no longer has.
    fn write(
        &self,
        base: Option<&str>,
        uid: &str,
        content: &ObjectContent,
        renamed: &[(OccurrenceKey, OccurrenceKey)],
    ) -> String {
        let mut ops = Vec::new();
        let base_event: Option<Value> = base.and_then(|b| serde_json::from_str(b).ok());
        let base_content = base.and_then(|b| self.read(b).ok()).map(|r| r.content);
        let master_id = base_event
            .as_ref()
            .and_then(|e| str_field(e, "id"))
            .unwrap_or(MASTER)
            .to_owned();
        if let Some(desired) = &content.master {
            match (
                &base_event,
                base_content.as_ref().and_then(|c| c.master.as_ref()),
            ) {
                (Some(e), Some(bm)) => {
                    let body = patch(desired, Some(bm));
                    if !body.is_empty() {
                        ops.push(json!({
                            "op": "patch",
                            "id": master_id,
                            "etag": etag_of(e),
                            "body": Value::Object(body),
                        }));
                    }
                }
                _ => {
                    let mut body = patch(desired, None);
                    // Graph assigns the id; transactionId makes a retried
                    // create return the existing event instead of a duplicate.
                    body.insert("transactionId".into(), json!(uid));
                    ops.push(json!({"op": "create", "body": Value::Object(body)}));
                }
            }
            // Newly cancelled occurrences. (Graph cannot restore one.)
            let base_exdates = base_content
                .as_ref()
                .and_then(|c| c.master.as_ref())
                .and_then(|m| m.recurrence.as_ref())
                .map(|r| r.exdates.clone())
                .unwrap_or_default();
            for key in desired
                .recurrence
                .iter()
                .flat_map(|r| &r.exdates)
                .filter(|k| !base_exdates.contains(k))
            {
                ops.push(json!({"op": "cancelInstance", "key": key_json(key)}));
            }
        }
        if let (Some(desired), Some(m)) = (&content.master, &base_event) {
            // Exceptions the series no longer generates (it was cut short,
            // e.g. by "this and following"). After the master's guarded
            // change, since removing them changes the series' version too.
            for x in exceptions(m) {
                let Some(key) = instance_key(x, m) else {
                    continue;
                };
                let kept = content.overrides.iter().any(|o| {
                    o.recurrence_id.is_some_and(|k| {
                        k == key || renamed.iter().any(|(c, b)| *c == k && *b == key)
                    })
                });
                let generated = desired.recurrence.as_ref().is_some_and(|rec| {
                    calendar_core::recurrence::has_occurrence(
                        &desired.timing,
                        rec,
                        &key,
                        &chrono_tz::UTC,
                    )
                    .unwrap_or(true)
                });
                if !kept && !generated {
                    ops.push(json!({"op": "delete", "id": str_field(x, "id")}));
                }
            }
        }
        let base_overrides = base_content
            .as_ref()
            .map(|c| c.overrides.as_slice())
            .unwrap_or_default();
        for o in &content.overrides {
            let Some(key) = o.recurrence_id else { continue };
            let moved = renamed
                .iter()
                .any(|(current, b)| *current == key && *b != key);
            let existing = base_event
                .as_ref()
                .and_then(|m| exceptions(m).find(|x| instance_key(x, m) == Some(key)));
            let base_override = base_overrides.iter().find(|b| b.recurrence_id == Some(key));
            match (existing, base_override, moved) {
                (Some(x), Some(b), false) => {
                    let body = patch(o, Some(b));
                    if !body.is_empty() {
                        ops.push(json!({
                            "op": "patch",
                            "id": str_field(x, "id"),
                            "etag": etag_of(x),
                            "body": Value::Object(body),
                        }));
                    }
                }
                // Not an exception yet (or the series moved): patching the
                // occurrence turns it into one.
                _ => ops.push(json!({
                    "op": "patchInstance",
                    "key": key_json(&key),
                    "body": Value::Object(patch(o, None)),
                })),
            }
        }
        json!({"uid": uid, "ops": ops}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const MEETING: &str = r#"{"id":"AAMk1","@odata.etag":"W/\"1\"","iCalUId":"040000008200E0","subject":"Planning",
      "type":"singleInstance","originalStartTimeZone":"W. Europe Standard Time",
      "body":{"contentType":"html","content":"<html><body><p>Agenda&nbsp;&amp; notes</p><p>Line 2</p></body></html>"},
      "location":{"displayName":"Room 1","address":{"city":"Berlin"}},
      "start":{"dateTime":"2026-06-01T07:00:00.0000000","timeZone":"UTC"},
      "end":{"dateTime":"2026-06-01T08:00:00.0000000","timeZone":"UTC"},
      "attendees":[{"emailAddress":{"address":"bob@example.com"}}]}"#;

    /// Daily stand-up at 09:00 Berlin from 1 June, five times; the 3rd
    /// moved to 11:00, the 4th cancelled.
    const SERIES: &str = r#"{"id":"S","@odata.etag":"W/\"5\"","iCalUId":"040000S","subject":"Stand-up",
      "type":"seriesMaster","originalStartTimeZone":"W. Europe Standard Time",
      "start":{"dateTime":"2026-06-01T07:00:00.0000000","timeZone":"UTC"},
      "end":{"dateTime":"2026-06-01T07:15:00.0000000","timeZone":"UTC"},
      "recurrence":{"pattern":{"type":"daily","interval":1},
        "range":{"type":"numbered","startDate":"2026-06-01","numberOfOccurrences":5,"recurrenceTimeZone":"W. Europe Standard Time"}},
      "cancelledOccurrences":["OID.S.2026-06-04"],
      "exceptionOccurrences":[{"id":"X3","changeKey":"7","subject":"Stand-up (late)",
        "type":"exception","seriesMasterId":"S","originalStart":"2026-06-03T07:00:00Z",
        "start":{"dateTime":"2026-06-03T09:00:00.0000000","timeZone":"UTC"},
        "end":{"dateTime":"2026-06-03T09:15:00.0000000","timeZone":"UTC"}}]}"#;

    fn berlin(d: u32, h: u32) -> DateTime<Utc> {
        chrono_tz::Europe::Berlin
            .with_ymd_and_hms(2026, 6, d, h, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ops(plan: &str) -> Vec<Value> {
        serde_json::from_str::<Value>(plan).unwrap()["ops"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn reads_times_zones_and_html_bodies() {
        let read = GraphCodec.read(MEETING).unwrap();
        let m = read.content.master.unwrap();
        assert_eq!(m.title, "Planning");
        assert_eq!(m.description, "Agenda & notes\nLine 2");
        assert_eq!(m.location, "Room 1");
        let EventTiming::Zoned {
            start, time_zone, ..
        } = m.timing
        else {
            panic!()
        };
        assert_eq!(time_zone, chrono_tz::Europe::Berlin);
        assert_eq!(start.to_rfc3339(), "2026-06-01T07:00:00+00:00");
        assert!(read.warnings.is_empty());
    }

    #[test]
    fn reads_series_exceptions_and_cancellations() {
        let read = GraphCodec.read(SERIES).unwrap();
        assert!(read.warnings.is_empty(), "{:?}", read.warnings);
        let c = read.content;
        let m = c.master.unwrap();
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

    #[test]
    fn writes_patches_and_creates() {
        let base = GraphCodec.read(MEETING).unwrap().content;
        let mut edited = base.clone();
        edited.master.as_mut().unwrap().title = "Planning (moved)".into();
        let plan = ops(&GraphCodec.write(Some(MEETING), "u", &edited, &[]));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0]["op"], "patch");
        assert_eq!(plan[0]["id"], "AAMk1");
        assert_eq!(plan[0]["etag"], "W/\"1\"");
        assert_eq!(plan[0]["body"], json!({"subject": "Planning (moved)"}));
        let plan = ops(&GraphCodec.write(None, "local-1", &edited, &[]));
        assert_eq!(plan[0]["op"], "create");
        assert_eq!(plan[0]["body"]["transactionId"], "local-1");
        // Local time in the event's zone, under its Windows name.
        assert_eq!(
            plan[0]["body"]["start"],
            json!({"dateTime": "2026-06-01T09:00:00", "timeZone": "W. Europe Standard Time"})
        );
        assert!(plan[0]["body"].get("recurrence").is_none());
    }

    #[test]
    fn writes_series_changes() {
        let base = GraphCodec.read(SERIES).unwrap().content;
        let mut edited = base.clone();
        {
            let m = edited.master.as_mut().unwrap();
            m.title = "Daily".into();
            let rec = m.recurrence.as_mut().unwrap();
            rec.rrule = "FREQ=DAILY;COUNT=10".into();
            rec.exdates.push(OccurrenceKey::Instant(berlin(5, 9)));
        }
        edited.overrides[0].location = "Room 2".into();
        let mut new = base.overrides[0].clone();
        new.recurrence_id = Some(OccurrenceKey::Instant(berlin(2, 9)));
        new.title = "Stand-up (short)".into();
        edited.overrides.push(new);
        let plan = ops(&GraphCodec.write(Some(SERIES), "u", &edited, &[]));
        assert_eq!(plan.len(), 4, "{plan:?}");
        assert_eq!(plan[0]["op"], "patch");
        assert_eq!(plan[0]["id"], "S");
        assert_eq!(plan[0]["body"]["subject"], "Daily");
        assert_eq!(
            plan[0]["body"]["recurrence"]["range"]["numberOfOccurrences"],
            10
        );
        assert_eq!(plan[1]["op"], "cancelInstance");
        assert_eq!(plan[1]["key"]["instant"], berlin(5, 9).to_rfc3339());
        assert_eq!(plan[2]["op"], "patch");
        assert_eq!(plan[2]["id"], "X3");
        assert_eq!(plan[2]["etag"], "W/\"7\"");
        assert_eq!(
            plan[2]["body"],
            json!({"location": {"displayName": "Room 2"}})
        );
        assert_eq!(plan[3]["op"], "patchInstance");
        assert_eq!(plan[3]["body"]["subject"], "Stand-up (short)");

        // A new series: create with its pattern.
        let plan = ops(&GraphCodec.write(None, "local-2", &base, &[]));
        assert_eq!(plan[0]["op"], "create");
        assert_eq!(plan[0]["body"]["recurrence"]["pattern"]["type"], "daily");
        assert_eq!(
            plan[0]["body"]["recurrence"]["range"]["startDate"],
            "2026-06-01"
        );
    }

    #[test]
    fn all_day_events() {
        let data = r#"{"id":"d","subject":"Holiday","isAllDay":true,
          "start":{"dateTime":"2026-12-25T00:00:00.0000000","timeZone":"UTC"},
          "end":{"dateTime":"2026-12-26T00:00:00.0000000","timeZone":"UTC"}}"#;
        let m = GraphCodec.read(data).unwrap().content.master.unwrap();
        assert!(
            matches!(m.timing, EventTiming::AllDay { start_date, .. } if start_date == NaiveDate::from_ymd_opt(2026, 12, 25).unwrap())
        );
        assert!(GraphCodec.read("[]").is_err());
        // An all-day slot given as midnight in a zone west of UTC still
        // names its own day.
        let timing = EventTiming::AllDay {
            start_date: NaiveDate::from_ymd_opt(2026, 12, 25).unwrap(),
            end_date: NaiveDate::from_ymd_opt(2026, 12, 26).unwrap(),
        };
        let ny = chrono_tz::America::New_York;
        for t in ["2026-12-25T00:00:00Z", "2026-12-25T05:00:00Z"] {
            let t = DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&Utc);
            assert_eq!(
                key_at(t, &timing, Some(ny)),
                OccurrenceKey::Date(NaiveDate::from_ymd_opt(2026, 12, 25).unwrap())
            );
        }
    }
}
