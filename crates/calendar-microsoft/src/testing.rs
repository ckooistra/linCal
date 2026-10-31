//! A small stateful imitation of the Microsoft Graph calendar API, enough
//! for provider and engine tests: `/me`, calendar list, calendarView delta
//! with paging, delta links, `@removed` entries and 410 for expired delta
//! state, event get/create (with `transactionId`)/patch/delete with ETags,
//! and 401 for expired access tokens.
//!
//! Series: masters (with daily or weekly patterns) are reported in the
//! delta by one occurrence each, as Graph reports instances; a master read
//! with `$expand` carries its exceptions and cancelled occurrences;
//! `instances` lists occurrences with ids `<master>.<date>`, which can be
//! patched (becoming exceptions) or deleted (cancelled).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[derive(Default)]
pub struct State {
    /// calendar id → event id → (change sequence, event JSON)
    pub events: BTreeMap<String, BTreeMap<String, (u64, Value)>>,
    /// Deleted event ids with the sequence of their deletion.
    pub removed: BTreeMap<String, Vec<(u64, String)>>,
    pub seq: u64,
    /// Delta tokens older than this answer 410.
    pub min_token: u64,
    pub valid_token: String,
    pub page_size: usize,
    pub next_id: u64,
    pub requests: Vec<String>,
    /// Series master id → cancelled dates (`yyyy-mm-dd`).
    pub cancelled: BTreeMap<String, Vec<String>>,
}

impl State {
    pub fn put(&mut self, calendar: &str, mut event: Value) {
        self.seq += 1;
        let id = event["id"].as_str().unwrap().to_owned();
        event["@odata.etag"] = json!(format!("W/\"{}\"", self.seq));
        self.events
            .entry(calendar.into())
            .or_default()
            .insert(id, (self.seq, event));
    }

    pub fn remove(&mut self, calendar: &str, id: &str) {
        self.seq += 1;
        if let Some(events) = self.events.get_mut(calendar) {
            events.remove(id);
        }
        let seq = self.seq;
        self.removed
            .entry(calendar.into())
            .or_default()
            .push((seq, id.into()));
    }

    /// A copy of a stored event, with the change counter bumped.
    pub fn touch(&mut self, id: &str) {
        if let Some((cal, e)) = self.find(id) {
            self.put(&cal, e);
        }
    }

    fn find(&self, id: &str) -> Option<(String, Value)> {
        self.events
            .iter()
            .find_map(|(cal, events)| events.get(id).map(|(_, e)| (cal.clone(), e.clone())))
    }
}

fn utc(t: &Value) -> Option<chrono::NaiveDateTime> {
    let raw = t["dateTime"].as_str()?;
    let naive =
        chrono::NaiveDateTime::parse_from_str(raw.split('.').next()?, "%Y-%m-%dT%H:%M:%S").ok()?;
    let zone = t["timeZone"].as_str().unwrap_or("UTC");
    let tz = calendar_ical::values::resolve_tzid(zone, &calendar_ical::Component::new("X"))?;
    Some(calendar_core::timing::resolve_local(&tz, naive).naive_utc())
}

fn utc_value(t: chrono::NaiveDateTime) -> Value {
    json!({"dateTime": format!("{}.0000000", t.format("%Y-%m-%dT%H:%M:%S")), "timeZone": "UTC"})
}

/// Stores times in UTC, as Graph returns them with the Prefer header, and
/// remembers the zone they were given in.
fn normalise_times(e: &mut Value) {
    for key in ["start", "end"] {
        let zone = e[key]["timeZone"].as_str().unwrap_or("UTC").to_owned();
        if zone != "UTC"
            && let Some(t) = utc(&e[key])
        {
            if key == "start" {
                e["originalStartTimeZone"] = json!(zone);
            }
            e[key] = utc_value(t);
        }
    }
}

/// Occurrences of a daily/weekly series master (exceptions not applied).
fn occurrences(master: &Value) -> Vec<(chrono::NaiveDateTime, chrono::Duration)> {
    let (Some(start), Some(end)) = (utc(&master["start"]), utc(&master["end"])) else {
        return Vec::new();
    };
    let pattern = &master["recurrence"]["pattern"];
    let range = &master["recurrence"]["range"];
    let interval = pattern["interval"].as_i64().unwrap_or(1);
    let step = match pattern["type"].as_str() {
        Some("weekly") => 7 * interval,
        _ => interval,
    };
    let count = match range["type"].as_str() {
        Some("numbered") => range["numberOfOccurrences"].as_i64().unwrap_or(1),
        _ => 400,
    };
    let until = range["endDate"]
        .as_str()
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    (0..count)
        .map(|n| start + chrono::Duration::days(n * step))
        .take_while(|t| until.is_none_or(|u| t.date() <= u))
        .map(|t| (t, end - start))
        .collect()
}

pub struct FakeGraph {
    pub server: MockServer,
    pub state: Arc<Mutex<State>>,
}

struct Handler(Arc<Mutex<State>>, String);

fn json_response(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
}

fn error(status: u16, code: &str) -> ResponseTemplate {
    json_response(status, json!({"error": {"code": code}}))
}

impl Respond for Handler {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut s = self.0.lock().unwrap();
        let base = &self.1;
        s.requests
            .push(format!("{} {}", req.method, req.url.path()));
        if req.url.path() == "/token" {
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
            return error(401, "InvalidAuthenticationToken");
        }
        let q: BTreeMap<String, String> = req.url.query_pairs().into_owned().collect();
        let path: Vec<&str> = req.url.path().trim_start_matches('/').split('/').collect();
        let page = |items: Vec<Value>, start: usize, size: usize, next: String, end: Value| {
            let total = items.len();
            let slice: Vec<Value> = items.into_iter().skip(start).take(size).collect();
            let mut body = json!({"value": slice});
            if start + size < total {
                body["@odata.nextLink"] = json!(next);
            } else {
                body["@odata.deltaLink"] = end;
            }
            body
        };
        match (req.method.as_str(), path.as_slice()) {
            ("GET", ["me"]) => json_response(200, json!({"mail": "carol@example.com"})),
            ("GET", ["me", "calendars"]) => json_response(
                200,
                json!({"value": [
                    {"id": "cal-1", "name": "Calendar", "hexColor": "#AABBCC", "canEdit": true, "changeKey": "k1"},
                    {"id": "cal-shared", "name": "Team", "canEdit": false}
                ]}),
            ),
            ("GET", ["me", "calendars", cal, "calendarView", "delta"]) => {
                let size = if s.page_size == 0 { 100 } else { s.page_size };
                let since: Option<u64> = match q.get("deltatoken") {
                    Some(t) => {
                        let t: u64 = t.parse().unwrap_or(0);
                        if t < s.min_token {
                            return error(410, "SyncStateNotFound");
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
                    .filter(|(_, (seq, _))| since.is_none_or(|t| *seq > t))
                    .map(|(_, (_, e))| {
                        if e["type"] == "seriesMaster" {
                            let id = e["id"].as_str().unwrap_or_default();
                            json!({"id": format!("{id}.first"), "type": "occurrence",
                                   "seriesMasterId": id, "start": e["start"], "end": e["end"]})
                        } else {
                            e.clone()
                        }
                    })
                    .collect();
                if let Some(t) = since {
                    for (seq, id) in s.removed.get(*cal).into_iter().flatten() {
                        if *seq > t {
                            items.push(json!({"id": id, "@removed": {"reason": "deleted"}}));
                        }
                    }
                }
                let start: usize = q.get("skip").and_then(|v| v.parse().ok()).unwrap_or(0);
                let mut next = url::Url::parse(&format!("{base}{}", req.url.path())).unwrap();
                for (k, v) in &q {
                    if k != "skip" {
                        next.query_pairs_mut().append_pair(k, v);
                    }
                }
                next.query_pairs_mut()
                    .append_pair("skip", &(start + size).to_string());
                let end = json!(format!(
                    "{base}/me/calendars/{cal}/calendarView/delta?deltatoken={}",
                    s.seq
                ));
                json_response(200, page(items, start, size, next.to_string(), end))
            }
            ("POST", ["me", "calendars", cal, "events"]) => {
                let cal = cal.to_string();
                let mut body: Value = serde_json::from_slice(&req.body).unwrap();
                // transactionId: a repeated create returns the first event.
                if let Some(tx) = body["transactionId"].as_str()
                    && let Some((_, e)) = s
                        .events
                        .get(&cal)
                        .into_iter()
                        .flatten()
                        .find(|(_, (_, e))| e["transactionId"].as_str() == Some(tx))
                {
                    return json_response(201, e.1.clone());
                }
                s.next_id += 1;
                let id = format!("AAMk{}", s.next_id);
                body["id"] = json!(id);
                body["type"] = json!(if body["recurrence"].is_object() {
                    "seriesMaster"
                } else {
                    "singleInstance"
                });
                normalise_times(&mut body);
                s.put(&cal, body);
                json_response(201, s.events[&cal][&id].1.clone())
            }
            ("GET", ["me", "events", id]) => match s.find(id) {
                Some((_, mut e)) => {
                    if e["type"] == "seriesMaster"
                        && q.get("$expand")
                            .is_some_and(|x| x == "exceptionOccurrences")
                    {
                        let exceptions: Vec<Value> = s
                            .events
                            .values()
                            .flat_map(|events| events.values())
                            .filter(|(_, x)| x["seriesMasterId"].as_str() == Some(*id))
                            .map(|(seq, x)| {
                                let mut x = x.clone();
                                // Expanded events carry a change key, no ETag.
                                x.as_object_mut().unwrap().remove("@odata.etag");
                                x["changeKey"] = json!(seq.to_string());
                                x
                            })
                            .collect();
                        e["exceptionOccurrences"] = json!(exceptions);
                        let cancelled: Vec<String> = s
                            .cancelled
                            .get(*id)
                            .into_iter()
                            .flatten()
                            .map(|d| format!("OID.{id}.{d}"))
                            .collect();
                        e["cancelledOccurrences"] = json!(cancelled);
                    }
                    json_response(200, e)
                }
                None => error(404, "ErrorItemNotFound"),
            },
            ("GET", ["me", "events", id, "instances"]) => {
                let Some((_, master)) = s.find(id) else {
                    return error(404, "ErrorItemNotFound");
                };
                let bound = |k: &str| {
                    q.get(k).and_then(|v| {
                        chrono::NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%SZ").ok()
                    })
                };
                let (from, to) = (bound("startDateTime"), bound("endDateTime"));
                let cancelled = s.cancelled.get(*id).cloned().unwrap_or_default();
                let mut items = Vec::new();
                for (start, length) in occurrences(&master) {
                    let date = start.date().format("%Y-%m-%d").to_string();
                    let instance_id = format!("{id}.{date}");
                    if cancelled.contains(&date)
                        || from.is_some_and(|f| start + length <= f)
                        || to.is_some_and(|t| start >= t)
                    {
                        continue;
                    }
                    let original = format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S"));
                    items.push(match s.find(&instance_id) {
                        Some((_, x)) => x,
                        None => json!({
                            "id": instance_id, "type": "occurrence", "seriesMasterId": id,
                            "originalStart": original, "subject": master["subject"],
                            "start": utc_value(start), "end": utc_value(start + length),
                        }),
                    });
                }
                json_response(200, json!({"value": items}))
            }
            ("PATCH", ["me", "events", id]) => {
                let found = s.find(id).or_else(|| {
                    // An occurrence: patching it creates the exception.
                    let (master_id, date) = id.rsplit_once('.')?;
                    let (cal, master) = s.find(master_id)?;
                    let (start, length) = occurrences(&master)
                        .into_iter()
                        .find(|(t, _)| t.date().format("%Y-%m-%d").to_string() == date)?;
                    let mut x = master.clone();
                    let x = x.as_object_mut()?;
                    x.remove("recurrence");
                    x.remove("@odata.etag");
                    let mut x = Value::Object(x.clone());
                    x["id"] = json!(id);
                    x["type"] = json!("occurrence");
                    x["seriesMasterId"] = json!(master_id);
                    x["originalStart"] = json!(format!("{}Z", start.format("%Y-%m-%dT%H:%M:%S")));
                    x["start"] = utc_value(start);
                    x["end"] = utc_value(start + length);
                    Some((cal, x))
                });
                let Some((cal, mut e)) = found else {
                    return error(404, "ErrorItemNotFound");
                };
                if let Some(m) = req.headers.get("if-match").and_then(|v| v.to_str().ok())
                    && e["@odata.etag"].as_str() != Some(m)
                {
                    return error(412, "ErrorIrresolvableConflict");
                }
                let patch: Value = serde_json::from_slice(&req.body).unwrap();
                for (k, v) in patch.as_object().unwrap() {
                    e[k] = v.clone();
                }
                normalise_times(&mut e);
                if e["recurrence"].is_null() && e["type"] == "seriesMaster" {
                    e["type"] = json!("singleInstance");
                }
                if e["type"] == "occurrence" {
                    e["type"] = json!("exception");
                }
                s.put(&cal, e);
                let id = id.to_string();
                json_response(200, s.find(&id).unwrap().1)
            }
            ("DELETE", ["me", "events", id]) => {
                if let Some((master_id, date)) = id.rsplit_once('.')
                    && let Some((cal, _)) = s.find(master_id)
                {
                    // Cancelling an occurrence (or an exception).
                    let (master_id, date, id) =
                        (master_id.to_owned(), date.to_owned(), id.to_string());
                    if let Some(events) = s.events.get_mut(&cal) {
                        events.remove(&id);
                    }
                    s.cancelled.entry(master_id.clone()).or_default().push(date);
                    s.seq += 1;
                    let seq = s.seq;
                    s.removed.entry(cal).or_default().push((seq, id));
                    s.touch(&master_id);
                    return ResponseTemplate::new(204);
                }
                let Some((cal, e)) = s.find(id) else {
                    return error(404, "ErrorItemNotFound");
                };
                if let Some(m) = req.headers.get("if-match").and_then(|v| v.to_str().ok())
                    && e["@odata.etag"].as_str() != Some(m)
                {
                    return error(412, "ErrorIrresolvableConflict");
                }
                let id = id.to_string();
                let exceptions: Vec<String> = s.events[&cal]
                    .iter()
                    .filter(|(_, (_, x))| x["seriesMasterId"].as_str() == Some(id.as_str()))
                    .map(|(x, _)| x.clone())
                    .collect();
                for x in exceptions {
                    s.remove(&cal, &x);
                }
                s.remove(&cal, &id);
                ResponseTemplate::new(204)
            }
            _ => error(400, "BadRequest"),
        }
    }
}

impl FakeGraph {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(State {
            valid_token: "token-1".into(),
            ..State::default()
        }));
        Mock::given(wiremock::matchers::any())
            .respond_with(Handler(Arc::clone(&state), server.uri()))
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

/// A Graph event in a calendar view (times in UTC).
pub fn event(id: &str, subject: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id, "subject": subject, "type": "singleInstance",
        "originalStartTimeZone": "W. Europe Standard Time",
        "start": {"dateTime": format!("{start}.0000000"), "timeZone": "UTC"},
        "end": {"dateTime": format!("{end}.0000000"), "timeZone": "UTC"},
        "body": {"contentType": "text", "content": ""},
        "attendees": [{"emailAddress": {"address": "dave@example.com"}}]
    })
}
