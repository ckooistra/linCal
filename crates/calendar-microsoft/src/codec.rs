//! Microsoft Graph event JSON ⇄ the provider-neutral model.
//!
//! Graph's calendar-view delta reports *instances*: single events,
//! occurrences and exceptions of series, each with its own id. Each is one
//! object here, read as a single (non-recurring) event. Changes are written
//! as patches of only the fields this app models, so attendees, online
//! meeting data, categories and the rest stay untouched. Times are
//! requested and written in UTC (`Prefer: outlook.timezone="UTC"`); the
//! event's own zone (`originalStartTimeZone`, Windows or IANA name) is kept
//! for display.

use calendar_core::{EventContent, EventTiming, ObjectContent, OccurrenceKey};
use calendar_provider_api::{ObjectCodec, ReadObject};
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Map, Value, json};

pub struct GraphCodec;

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
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

/// One Graph event as provider-neutral content.
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
        let zone = str_field(e, "originalStartTimeZone").unwrap_or("UTC");
        let tz =
            calendar_ical::values::resolve_tzid(zone, &calendar_ical::Component::new("VCALENDAR"))
                .unwrap_or_else(|| {
                    warnings.push(format!("unknown time zone {zone:?}; shown in UTC"));
                    chrono_tz::UTC
                });
        let start = start.and_utc();
        EventTiming::Zoned {
            start,
            end: end.and_utc().max(start),
            time_zone: tz,
        }
    };
    if str_field(e, "type").is_some_and(|t| t == "occurrence" || t == "exception") {
        // One occurrence of a series; the series itself is not modelled.
        warnings.push(
            "occurrence of a Microsoft series; whole-series editing is not supported yet".into(),
        );
    }
    Ok((
        EventContent {
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

fn time_value(timing: &EventTiming, start: bool) -> (Value, bool) {
    match timing {
        EventTiming::Zoned {
            start: s, end: e, ..
        } => {
            let t = if start { s } else { e };
            (
                json!({"dateTime": t.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"}),
                false,
            )
        }
        EventTiming::Floating { start: s, end: e } => {
            let t = if start { s } else { e };
            (
                json!({"dateTime": t.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"}),
                false,
            )
        }
        EventTiming::AllDay {
            start_date,
            end_date,
        } => {
            let d: NaiveDate = if start { *start_date } else { *end_date };
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
    if base.map(|b| &b.timing) != Some(&desired.timing) {
        let (start, all_day) = time_value(&desired.timing, true);
        body.insert("start".into(), start);
        body.insert("end".into(), time_value(&desired.timing, false).0);
        body.insert("isAllDay".into(), json!(all_day));
        if let EventTiming::Zoned { time_zone, .. } = &desired.timing {
            body.insert("originalStartTimeZone".into(), json!(time_zone.name()));
            body.insert("originalEndTimeZone".into(), json!(time_zone.name()));
        }
    }
    body
}

impl ObjectCodec for GraphCodec {
    fn read(&self, data: &str) -> Result<ReadObject, String> {
        let e: Value =
            serde_json::from_str(data).map_err(|err| format!("not Graph event data: {err}"))?;
        let uid = str_field(&e, "iCalUId")
            .or(str_field(&e, "id"))
            .ok_or("event without an id")?
            .to_owned();
        let (content, warnings) = event_content(&e)?;
        Ok(ReadObject {
            content: ObjectContent {
                uid,
                master: Some(content),
                overrides: Vec::new(),
            },
            warnings,
        })
    }

    /// `{"uid": …, "method": "POST"|"PATCH", "etag": …, "body": {…}}`
    fn write(
        &self,
        base: Option<&str>,
        uid: &str,
        content: &ObjectContent,
        _renamed: &[(OccurrenceKey, OccurrenceKey)],
    ) -> String {
        let Some(desired) = &content.master else {
            return json!({"uid": uid, "method": "NONE"}).to_string();
        };
        let base_event: Option<Value> = base.and_then(|b| serde_json::from_str(b).ok());
        match base_event {
            Some(e) => {
                let base_content = event_content(&e).ok().map(|(c, _)| c);
                json!({
                    "uid": uid,
                    "method": "PATCH",
                    "etag": e.get("@odata.etag"),
                    "body": Value::Object(patch(desired, base_content.as_ref())),
                })
                .to_string()
            }
            None => {
                let mut body = patch(desired, None);
                // Graph assigns the id; transactionId makes a retried create
                // return the existing event instead of a duplicate.
                body.insert("transactionId".into(), json!(uid));
                json!({"uid": uid, "method": "POST", "body": Value::Object(body)}).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEETING: &str = r#"{"id":"AAMk1","@odata.etag":"W/\"1\"","iCalUId":"040000008200E0","subject":"Planning",
      "type":"singleInstance","originalStartTimeZone":"W. Europe Standard Time",
      "body":{"contentType":"html","content":"<html><body><p>Agenda&nbsp;&amp; notes</p><p>Line 2</p></body></html>"},
      "location":{"displayName":"Room 1","address":{"city":"Berlin"}},
      "start":{"dateTime":"2026-06-01T07:00:00.0000000","timeZone":"UTC"},
      "end":{"dateTime":"2026-06-01T08:00:00.0000000","timeZone":"UTC"},
      "attendees":[{"emailAddress":{"address":"bob@example.com"}}]}"#;

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
    fn writes_patches_and_creates() {
        let base = GraphCodec.read(MEETING).unwrap().content;
        let mut edited = base.clone();
        edited.master.as_mut().unwrap().title = "Planning (moved)".into();
        let plan: Value =
            serde_json::from_str(&GraphCodec.write(Some(MEETING), "u", &edited, &[])).unwrap();
        assert_eq!(plan["method"], "PATCH");
        assert_eq!(plan["etag"], "W/\"1\"");
        assert_eq!(plan["body"], json!({"subject": "Planning (moved)"}));
        let plan: Value =
            serde_json::from_str(&GraphCodec.write(None, "local-1", &edited, &[])).unwrap();
        assert_eq!(plan["method"], "POST");
        assert_eq!(plan["body"]["transactionId"], "local-1");
        assert_eq!(plan["body"]["start"]["timeZone"], "UTC");
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
    }
}
