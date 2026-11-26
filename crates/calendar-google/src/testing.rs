//! A small stateful imitation of the Google Calendar API v3, enough for
//! provider and engine tests: calendar list, events list with sync tokens
//! and paging, get/insert/patch/delete with ETags, instance ids, 410 for
//! expired sync tokens, and 401 for expired access tokens.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
pub struct State {
    /// calendar id → event id → (change sequence, event JSON)
    pub events: BTreeMap<String, BTreeMap<String, (u64, Value)>>,
    pub seq: u64,
    /// Sync tokens older than this answer 410.
    pub min_token: u64,
    pub valid_token: String,
    pub requests: Vec<String>,
    pub page_size: usize,
    /// Also list a second calendar Alice owns, "team@group".
    pub team_calendar: bool,
}

/// Google answers times with their offset, also when they were written as
/// a wall-clock time with a `timeZone`.
fn with_offsets(event: &mut Value) {
    for field in ["start", "end", "originalStartTime"] {
        let Some(t) = event.get_mut(field) else {
            continue;
        };
        let (Some(local), Some(zone)) = (
            t["dateTime"]
                .as_str()
                .and_then(|d| chrono::NaiveDateTime::parse_from_str(d, "%Y-%m-%dT%H:%M:%S").ok()),
            t["timeZone"]
                .as_str()
                .and_then(|z| z.parse::<chrono_tz::Tz>().ok()),
        ) else {
            continue;
        };
        let at = calendar_core::timing::resolve_local(&zone, local).with_timezone(&zone);
        t["dateTime"] = json!(at.to_rfc3339());
    }
}

impl State {
    pub fn put(&mut self, calendar: &str, mut event: Value) {
        with_offsets(&mut event);
        self.seq += 1;
        let id = event["id"].as_str().unwrap().to_owned();
        event["etag"] = json!(format!("\"{}\"", self.seq));
        if event.get("status").is_none() {
            event["status"] = json!("confirmed");
        }
        self.events
            .entry(calendar.into())
            .or_default()
            .insert(id, (self.seq, event));
    }
}

pub struct FakeGoogle {
    pub server: MockServer,
    pub state: Arc<Mutex<State>>,
}

struct Handler(Arc<Mutex<State>>);

fn json_response(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
}

fn error(status: u16, reason: &str) -> ResponseTemplate {
    json_response(
        status,
        json!({"error": {"code": status, "errors": [{"reason": reason}]}}),
    )
}

fn decode(segment: &str) -> String {
    url::form_urlencoded::parse(format!("x={segment}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

impl Respond for Handler {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut s = self.0.lock().unwrap();
        s.requests
            .push(format!("{} {}", req.method, req.url.path()));
        if req.url.path() == "/token" {
            // Refresh: issue the currently valid token.
            return json_response(
                200,
                json!({"access_token": s.valid_token, "expires_in": 3600}),
            );
        }
        if req
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            != Some(&format!("Bearer {}", s.valid_token))
        {
            return error(401, "authError");
        }
        let q: BTreeMap<String, String> = req.url.query_pairs().into_owned().collect();
        let path: Vec<String> = req
            .url
            .path()
            .trim_start_matches('/')
            .split('/')
            .map(decode)
            .collect();
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        match (req.method.as_str(), path.as_slice()) {
            ("GET", ["users", "me", "calendarList"]) => {
                let mut items = json!([
                    {"id": "alice@example.com", "summary": "Alice", "accessRole": "owner", "primary": true, "backgroundColor": "#9FC6E7", "etag": "\"c1\"", "timeZone": "Europe/Berlin",
                     "defaultReminders": [{"method": "popup", "minutes": 10}, {"method": "email", "minutes": 60}]},
                    {"id": "holidays@group", "summary": "Holidays", "accessRole": "reader"},
                    {"id": "busy@group", "summary": "Busy", "accessRole": "freeBusyReader"}
                ]);
                if s.team_calendar {
                    items.as_array_mut().unwrap().push(
                        json!({"id": "team@group", "summary": "Team", "accessRole": "owner"}),
                    );
                }
                json_response(200, json!({ "items": items }))
            }
            ("GET", ["users", "me", "calendarList", cal]) => match *cal {
                "team@group" if s.team_calendar => {
                    json_response(200, json!({"id": cal, "defaultReminders": []}))
                }
                "alice@example.com" => json_response(
                    200,
                    json!({"id": cal, "timeZone": "Europe/Berlin", "defaultReminders": [{"method": "popup", "minutes": 10}]}),
                ),
                _ => error(404, "notFound"),
            },
            ("GET", ["calendars", cal]) if *cal == "primary" => json_response(
                200,
                json!({"id": "alice@example.com", "timeZone": "Europe/Berlin"}),
            ),
            ("GET", ["calendars", cal, "events"]) => {
                let since = match q.get("syncToken") {
                    Some(t) => {
                        let t: u64 = t.trim_start_matches('t').parse().unwrap_or(0);
                        if t < s.min_token {
                            return error(410, "fullSyncRequired");
                        }
                        Some(t)
                    }
                    None => None,
                };
                let mut items: Vec<Value> = s
                    .events
                    .get(*cal)
                    .into_iter()
                    .flatten()
                    .filter(|(_, (seq, e))| match since {
                        Some(t) => *seq > t,
                        None => q.contains_key("showDeleted") || e["status"] != "cancelled",
                    })
                    .filter(|(_, (_, e))| match q.get("iCalUID") {
                        Some(uid) => e["iCalUID"].as_str() == Some(uid),
                        None => true,
                    })
                    .map(|(_, (_, e))| e.clone())
                    .collect();
                let start: usize = q.get("pageToken").and_then(|p| p.parse().ok()).unwrap_or(0);
                let size = if s.page_size == 0 { 250 } else { s.page_size };
                let total = items.len();
                let page: Vec<Value> = items.drain(..).skip(start).take(size).collect();
                let mut body = json!({"timeZone": "Europe/Berlin", "items": page});
                if start + size < total {
                    body["nextPageToken"] = json!((start + size).to_string());
                } else {
                    body["nextSyncToken"] = json!(format!("t{}", s.seq));
                }
                json_response(200, body)
            }
            ("GET", ["calendars", cal, "events", id]) => {
                match s.events.get(*cal).and_then(|m| m.get(*id)) {
                    Some((_, e)) => json_response(200, e.clone()),
                    None => error(404, "notFound"),
                }
            }
            ("POST", ["calendars", cal, "events"]) => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                // Google's limits on reminders: five, up to four weeks.
                let overrides = body["reminders"]["overrides"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if overrides.len() > 5
                    || overrides
                        .iter()
                        .any(|o| o["minutes"].as_u64() > Some(40320))
                {
                    return json_response(
                        400,
                        json!({"error": {"code": 400, "message": "Invalid value for: Reminders override minutes",
                               "errors": [{"reason": "invalid", "message": "Invalid value for: Reminders override minutes"}]}}),
                    );
                }
                let id = body["id"].as_str().unwrap().to_owned();
                if s.events.get(*cal).is_some_and(|m| m.contains_key(&id)) {
                    return error(409, "duplicate");
                }
                let cal = cal.to_string();
                s.put(&cal, body);
                json_response(200, s.events[&cal][&id].1.clone())
            }
            ("POST", ["calendars", cal, "events", id, "move"]) => {
                // The series with its exceptions; the source lists them as
                // cancelled from now on, as Google does.
                let (cal, id) = (cal.to_string(), id.to_string());
                let Some(to) = q.get("destination").cloned() else {
                    return error(400, "required");
                };
                if s.events.get(&cal).and_then(|m| m.get(&id)).is_none() {
                    return error(404, "notFound");
                }
                if s.events.get(&cal).unwrap()[&id].1["organizer"]["self"] == json!(false) {
                    return error(400, "cannotChangeOrganizer");
                }
                let members: Vec<Value> = s.events[&cal]
                    .values()
                    .map(|(_, e)| e.clone())
                    .filter(|e| e["id"] == json!(id) || e["recurringEventId"] == json!(id))
                    .collect();
                for e in members {
                    let mut gone = e.clone();
                    gone["status"] = json!("cancelled");
                    s.put(&cal, gone);
                    s.put(&to, e);
                }
                json_response(200, s.events[&to][&id].1.clone())
            }
            ("PATCH", ["calendars", cal, "events", id]) => {
                let cal = cal.to_string();
                let patch: Value = serde_json::from_slice(&req.body).unwrap();
                let current = s
                    .events
                    .get(&cal)
                    .and_then(|m| m.get(*id))
                    .map(|(_, e)| e.clone());
                let if_match = req.headers.get("if-match").and_then(|v| v.to_str().ok());
                let mut event = match current {
                    Some(e) => {
                        if let Some(m) = if_match
                            && e["etag"].as_str() != Some(m)
                        {
                            return error(412, "conditionNotMet");
                        }
                        e
                    }
                    None => {
                        // Patching an instance id creates the exception.
                        let Some((master, suffix)) = id.split_once('_') else {
                            return error(404, "notFound");
                        };
                        let Some((_, m)) = s.events.get(&cal).and_then(|c| c.get(master)).cloned()
                        else {
                            return error(404, "notFound");
                        };
                        let t = chrono::NaiveDateTime::parse_from_str(suffix, "%Y%m%dT%H%M%SZ")
                            .unwrap();
                        json!({
                            "id": id, "iCalUID": m["iCalUID"], "recurringEventId": master,
                            "originalStartTime": {"dateTime": format!("{}Z", t.format("%Y-%m-%dT%H:%M:%S")), "timeZone": "Europe/Berlin"},
                            "summary": m["summary"], "start": m["start"], "end": m["end"],
                        })
                    }
                };
                for (k, v) in patch.as_object().unwrap() {
                    event[k] = v.clone();
                }
                s.put(&cal, event);
                json_response(200, s.events[&cal][*id].1.clone())
            }
            ("DELETE", ["calendars", cal, "events", id]) => {
                let cal = cal.to_string();
                let Some((_, mut e)) = s.events.get(&cal).and_then(|m| m.get(*id)).cloned() else {
                    return error(410, "deleted");
                };
                if let Some(m) = req.headers.get("if-match").and_then(|v| v.to_str().ok())
                    && e["etag"].as_str() != Some(m)
                {
                    return error(412, "conditionNotMet");
                }
                e["status"] = json!("cancelled");
                s.put(&cal, e);
                ResponseTemplate::new(204)
            }
            _ => error(400, "unsupported"),
        }
    }
}

impl FakeGoogle {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(State {
            valid_token: "token-1".into(),
            ..State::default()
        }));
        Mock::given(wiremock::matchers::any())
            .respond_with(Handler(Arc::clone(&state)))
            .mount(&server)
            .await;
        Self { server, state }
    }

    pub fn api(&self) -> url::Url {
        url::Url::parse(&format!("{}/", self.server.uri())).unwrap()
    }

    pub fn token_url(&self) -> url::Url {
        url::Url::parse(&format!("{}/token", self.server.uri())).unwrap()
    }
}
