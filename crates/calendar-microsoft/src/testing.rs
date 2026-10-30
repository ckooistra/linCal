//! A small stateful imitation of the Microsoft Graph calendar API, enough
//! for provider and engine tests: `/me`, calendar list, calendarView delta
//! with paging, delta links, `@removed` entries and 410 for expired delta
//! state, event get/create (with `transactionId`)/patch/delete with ETags,
//! and 401 for expired access tokens.

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

    fn find(&self, id: &str) -> Option<(String, Value)> {
        self.events
            .iter()
            .find_map(|(cal, events)| events.get(id).map(|(_, e)| (cal.clone(), e.clone())))
    }
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
                    .map(|(_, (_, e))| e.clone())
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
                body["type"] = json!("singleInstance");
                s.put(&cal, body);
                json_response(201, s.events[&cal][&id].1.clone())
            }
            ("GET", ["me", "events", id]) => match s.find(id) {
                Some((_, e)) => json_response(200, e),
                None => error(404, "ErrorItemNotFound"),
            },
            ("PATCH", ["me", "events", id]) => {
                let Some((cal, mut e)) = s.find(id) else {
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
                if e["type"] == "occurrence" {
                    e["type"] = json!("exception");
                }
                s.put(&cal, e);
                let id = id.to_string();
                json_response(200, s.find(&id).unwrap().1)
            }
            ("DELETE", ["me", "events", id]) => {
                let Some((cal, e)) = s.find(id) else {
                    return error(404, "ErrorItemNotFound");
                };
                if let Some(m) = req.headers.get("if-match").and_then(|v| v.to_str().ok())
                    && e["@odata.etag"].as_str() != Some(m)
                {
                    return error(412, "ErrorIrresolvableConflict");
                }
                let id = id.to_string();
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
