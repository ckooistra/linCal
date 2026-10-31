//! Google Calendar API v3 provider.
//!
//! * Objects are whole series (master + exceptions), see [`codec`] and
//!   `docs/decisions/0005-google-and-microsoft.md`.
//! * hrefs: calendar `…/calendars/<id>/`, object `…/calendars/<id>/events/<master id>`.
//!   Objects created locally arrive as `…/calendars/<id>/<name>.ics`; their
//!   Google id is derived from `<name>` (client-chosen ids make a retried
//!   insert answer 409 instead of creating a duplicate).
//! * Incremental sync uses `syncToken`; HTTP 410 restarts with a full sync.
//! * Access tokens come from a [`TokenSource`] and are refreshed on expiry
//!   or a 401.

pub mod codec;
#[cfg(feature = "testing")]
pub mod testing;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use calendar_oauth::{OAuthClient, OAuthError, TokenSource};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, ObjectCodec, Precondition, ProviderError,
    ProviderResult, RemoteCalendar, RemoteObject, WriteResult,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use url::Url;

pub use codec::GoogleCodec;
use codec::{MASTER, is_cancelled, is_exception};

pub const API_BASE: &str = "https://www.googleapis.com/calendar/v3/";
pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

/// OAuth settings for a Google "Desktop app" client registration.
pub fn oauth_client(client_id: &str, client_secret: Option<&str>) -> OAuthClient {
    OAuthClient {
        auth_url: Url::parse("https://accounts.google.com/o/oauth2/v2/auth").expect("valid"),
        token_url: Url::parse("https://oauth2.googleapis.com/token").expect("valid"),
        client_id: client_id.to_owned(),
        client_secret: client_secret.map(str::to_owned),
        scopes: vec![SCOPE.to_owned()],
        // A refresh token, and the consent screen so one is always issued.
        extra_params: vec![
            ("access_type".into(), "offline".into()),
            ("prompt".into(), "consent".into()),
        ],
        redirect_host: "127.0.0.1".into(),
    }
}

/// What a Google account keeps in the OS keyring.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleSecret {
    pub tokens: calendar_oauth::Tokens,
    /// The (non-confidential) secret of a Google desktop client, if any.
    pub client_secret: Option<String>,
}

pub struct GoogleProvider {
    http: reqwest::Client,
    tokens: Arc<TokenSource>,
    api: Url,
    client_secret: Option<String>,
}

fn oauth_error(e: OAuthError) -> ProviderError {
    match e {
        OAuthError::Expired | OAuthError::Denied(_) => ProviderError::Unauthorized,
        OAuthError::Network(m) => ProviderError::Network(m),
        other => ProviderError::InvalidResponse(other.to_string()),
    }
}

fn invalid(what: &str) -> ProviderError {
    ProviderError::InvalidResponse(what.to_owned())
}

/// Google event ids are base32hex (`a-v`, `0-9`), 5–1024 characters.
fn event_id_for(name: &str) -> String {
    let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
    let id = format!("uc{hex}");
    id.chars().take(1024).collect()
}

/// `url` with query parameters added (kept out of async code: the
/// serializer is not `Send`).
fn with_query(url: &Url, pairs: &[(&str, &str)]) -> Url {
    let mut url = url.clone();
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in pairs {
            q.append_pair(k, v);
        }
    }
    url
}

/// Version of a series: changes when any member changes.
fn series_etag(events: &[Value]) -> String {
    let mut tags: Vec<&str> = events
        .iter()
        .filter_map(|e| e.get("etag").and_then(Value::as_str))
        .collect();
    tags.sort_unstable();
    let hash = tags
        .join("|")
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("\"g{hash:016x}\"")
}

fn series_data(events: &[Value], time_zone: Option<&str>) -> String {
    json!({ "timeZone": time_zone, "events": events }).to_string()
}

/// (calendar href, Google event id or `None` for a not-yet-created object).
fn split_href(href: &str) -> ProviderResult<(String, Option<String>, String)> {
    if let Some((calendar, id)) = href.split_once("/events/") {
        return Ok((format!("{calendar}/"), Some(id.to_owned()), id.to_owned()));
    }
    let (calendar, name) = href.rsplit_once('/').ok_or_else(|| invalid("bad href"))?;
    let name = name.strip_suffix(".ics").unwrap_or(name);
    Ok((format!("{calendar}/"), None, event_id_for(name)))
}

impl GoogleProvider {
    pub fn new(tokens: Arc<TokenSource>, api: Url) -> ProviderResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("UniversalCalendar/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok(Self {
            http,
            tokens,
            api,
            client_secret: None,
        })
    }

    /// A provider for an account from its client id and stored secret.
    pub fn connect(client_id: &str, secret: &str, api: Url) -> ProviderResult<Self> {
        let secret: GoogleSecret =
            serde_json::from_str(secret).map_err(|_| ProviderError::Unauthorized)?;
        let client = oauth_client(client_id, secret.client_secret.as_deref());
        let mut provider = Self::new(Arc::new(TokenSource::new(client, secret.tokens)), api)?;
        provider.client_secret = secret.client_secret;
        Ok(provider)
    }

    fn calendar_href(&self, calendar_id: &str) -> String {
        let mut url = self.api.clone();
        url.path_segments_mut()
            .expect("base URL")
            .pop_if_empty()
            .extend(["calendars", calendar_id, ""]);
        url.to_string()
    }

    /// Sends one API request. Returns the status and JSON body for success
    /// and for the statuses in `ok`; maps the rest to provider errors.
    async fn send(
        &self,
        method: Method,
        url: &str,
        if_match: Option<&str>,
        body: Option<&Value>,
        ok: &[StatusCode],
    ) -> ProviderResult<(StatusCode, Value)> {
        let mut token = self.tokens.access_token().await.map_err(oauth_error)?;
        for attempt in 0..2 {
            let mut request = self
                .http
                .request(method.clone(), url)
                .bearer_auth(&token)
                .header("Accept", "application/json");
            if let Some(etag) = if_match {
                request = request.header("If-Match", etag);
            }
            if let Some(b) = body {
                request = request
                    .header("Content-Type", "application/json")
                    .body(b.to_string());
            }
            let response = request.send().await.map_err(|e| {
                ProviderError::Network(if e.is_timeout() {
                    "timed out".into()
                } else {
                    "request failed".into()
                })
            })?;
            let status = response.status();
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let text = response
                .text()
                .await
                .map_err(|_| ProviderError::Network("response interrupted".into()))?;
            let json: Value = if text.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text).unwrap_or(Value::Null)
            };
            if status.is_success() || ok.contains(&status) {
                return Ok((status, json));
            }
            if status == StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self
                    .tokens
                    .refresh_after_rejection(&token)
                    .await
                    .map_err(oauth_error)?;
                continue;
            }
            let reason = json["error"]["errors"][0]["reason"]
                .as_str()
                .unwrap_or_default();
            return Err(match status.as_u16() {
                401 => ProviderError::Unauthorized,
                403 if reason.contains("RateLimit") || reason == "quotaExceeded" => {
                    ProviderError::RateLimited { retry_after }
                }
                403 => ProviderError::Forbidden(reason.to_owned()),
                404 | 410 => ProviderError::NotFound(url.to_owned()),
                409 | 412 => ProviderError::PreconditionFailed,
                429 => ProviderError::RateLimited { retry_after },
                s if s >= 500 => ProviderError::Server { status: s },
                s => ProviderError::InvalidResponse(format!("HTTP {s} {reason}")),
            });
        }
        Err(ProviderError::Unauthorized)
    }

    /// All pages of a list request; `on_page` receives each response.
    async fn pages(
        &self,
        url: Url,
        mut on_page: impl FnMut(&Value),
        ok: &[StatusCode],
    ) -> ProviderResult<StatusCode> {
        let mut page: Option<String> = None;
        for _ in 0..10_000 {
            let u = match &page {
                Some(p) => with_query(&url, &[("pageToken", p)]),
                None => url.clone(),
            };
            let (status, body) = self.send(Method::GET, u.as_str(), None, None, ok).await?;
            if !status.is_success() {
                return Ok(status);
            }
            on_page(&body);
            match body["nextPageToken"].as_str() {
                Some(next) => page = Some(next.to_owned()),
                None => return Ok(status),
            }
        }
        Err(invalid("too many pages"))
    }

    fn events_url(&self, calendar_href: &str) -> ProviderResult<Url> {
        Url::parse(&format!("{calendar_href}events")).map_err(|_| invalid("bad calendar URL"))
    }

    /// The current members of a series, or `None` if it no longer exists.
    async fn fetch_series(
        &self,
        calendar_href: &str,
        master_id: &str,
    ) -> ProviderResult<Option<(Vec<Value>, Option<String>)>> {
        let (status, master) = self
            .send(
                Method::GET,
                &format!("{calendar_href}events/{master_id}"),
                None,
                None,
                &[StatusCode::NOT_FOUND, StatusCode::GONE],
            )
            .await?;
        if !status.is_success() || is_cancelled(&master) {
            return Ok(None);
        }
        let Some(uid) = master["iCalUID"].as_str() else {
            return Ok(Some((vec![master], None)));
        };
        let url = with_query(
            &self.events_url(calendar_href)?,
            &[
                ("iCalUID", uid),
                ("showDeleted", "true"),
                ("maxResults", "250"),
            ],
        );
        let mut members = Vec::new();
        let mut zone = None;
        self.pages(
            url,
            |page| {
                zone = page["timeZone"].as_str().map(str::to_owned);
                for e in page["items"].as_array().into_iter().flatten() {
                    let id = e["id"].as_str();
                    let parent = e["recurringEventId"].as_str();
                    if id == Some(master_id) || parent == Some(master_id) {
                        members.push(e.clone());
                    }
                }
            },
            &[],
        )
        .await?;
        if !members.iter().any(|e| !is_exception(e)) {
            members.insert(0, master);
        }
        // Master first, then exceptions in a stable order.
        members.sort_by_key(|e| (is_exception(e), e["id"].as_str().map(str::to_owned)));
        Ok(Some((members, zone)))
    }

    fn object(
        &self,
        calendar_href: &str,
        master_id: &str,
        events: &[Value],
        zone: Option<&str>,
    ) -> RemoteObject {
        RemoteObject {
            href: format!("{calendar_href}events/{master_id}"),
            etag: Some(series_etag(events)),
            data: series_data(events, zone),
        }
    }

    /// The account's e-mail address (the id of the primary calendar).
    pub async fn account_email(&self) -> ProviderResult<String> {
        let (_, body) = self
            .send(
                Method::GET,
                self.calendar_href("primary").trim_end_matches('/'),
                None,
                None,
                &[],
            )
            .await?;
        body["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid("primary calendar without id"))
    }
}

#[async_trait]
impl CalendarProvider for GoogleProvider {
    fn codec(&self) -> Arc<dyn ObjectCodec> {
        Arc::new(GoogleCodec)
    }

    fn take_updated_secret(&self) -> Option<String> {
        let tokens = self.tokens.take_changed()?;
        serde_json::to_string(&GoogleSecret {
            tokens,
            client_secret: self.client_secret.clone(),
        })
        .ok()
    }

    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        let url = with_query(
            &self
                .api
                .join("users/me/calendarList")
                .map_err(|_| invalid("bad API URL"))?,
            &[("maxResults", "250")],
        );
        let mut out = Vec::new();
        self.pages(
            url,
            |page| {
                for c in page["items"].as_array().into_iter().flatten() {
                    let role = c["accessRole"].as_str().unwrap_or("reader");
                    let Some(id) = c["id"].as_str() else { continue };
                    // Free/busy-only calendars have no event details to show.
                    if role == "freeBusyReader" || c["deleted"].as_bool() == Some(true) {
                        continue;
                    }
                    out.push(RemoteCalendar {
                        href: self.calendar_href(id),
                        name: c["summaryOverride"]
                            .as_str()
                            .or(c["summary"].as_str())
                            .unwrap_or(id)
                            .to_owned(),
                        color: c["backgroundColor"]
                            .as_str()
                            .filter(|v| v.len() == 7 && v.starts_with('#'))
                            .map(str::to_ascii_lowercase),
                        read_only: !matches!(role, "owner" | "writer"),
                        ctag: c["etag"].as_str().map(str::to_owned),
                        capabilities: Capabilities {
                            incremental_sync: true,
                            conditional_writes: true,
                        },
                    });
                }
            },
            &[],
        )
        .await?;
        Ok(out)
    }

    async fn fetch_changes(
        &self,
        calendar: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        let mut cursor = cursor.map(str::to_owned);
        let (items, zone, next) = loop {
            let mut pairs = vec![("showDeleted", "true"), ("maxResults", "250")];
            if let Some(token) = &cursor {
                pairs.push(("syncToken", token));
            }
            let url = with_query(&self.events_url(&calendar.href)?, &pairs);
            let mut items = Vec::new();
            let mut zone = None;
            let mut next = None;
            let status = self
                .pages(
                    url,
                    |page| {
                        items.extend(page["items"].as_array().into_iter().flatten().cloned());
                        zone = page["timeZone"].as_str().map(str::to_owned).or(zone.take());
                        next = page["nextSyncToken"]
                            .as_str()
                            .map(str::to_owned)
                            .or(next.take());
                    },
                    &[StatusCode::GONE],
                )
                .await?;
            if status == StatusCode::GONE {
                // The sync token expired: start over with a full listing.
                if cursor.take().is_none() {
                    return Err(invalid("events list answered 410"));
                }
                continue;
            }
            break (items, zone, next);
        };

        let mut groups: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for e in items {
            let series = e["recurringEventId"]
                .as_str()
                .or(e["id"].as_str())
                .map(str::to_owned);
            if let Some(series) = series {
                groups.entry(series).or_default().push(e);
            }
        }
        let known: HashMap<&str, Option<&str>> = known
            .iter()
            .map(|(h, e)| (h.as_str(), e.as_deref()))
            .collect();
        let mut changes = ChangeSet {
            cursor: next,
            ..ChangeSet::default()
        };
        let href_of = |id: &str| format!("{}events/{id}", calendar.href);

        if cursor.is_none() {
            // Complete listing: every series is here with all its members.
            let mut listed = HashSet::new();
            for (id, mut events) in groups {
                let master_gone = events.iter().any(|e| !is_exception(e) && is_cancelled(e));
                if master_gone || events.iter().all(is_cancelled) {
                    continue;
                }
                events.sort_by_key(|e| (is_exception(e), e["id"].as_str().map(str::to_owned)));
                let object = self.object(&calendar.href, &id, &events, zone.as_deref());
                listed.insert(object.href.clone());
                if known.get(object.href.as_str()).copied().flatten() != object.etag.as_deref() {
                    changes.upserted.push(object);
                }
            }
            changes.deleted = known
                .keys()
                .filter(|h| !listed.contains(**h))
                .map(|h| (*h).to_owned())
                .collect();
        } else {
            // Changed members only: read each affected series again.
            for (id, events) in groups {
                let href = href_of(&id);
                if events.iter().any(|e| !is_exception(e) && is_cancelled(e)) {
                    if known.contains_key(href.as_str()) {
                        changes.deleted.push(href);
                    }
                    continue;
                }
                match self.fetch_series(&calendar.href, &id).await? {
                    Some((members, z)) => changes.upserted.push(self.object(
                        &calendar.href,
                        &id,
                        &members,
                        z.as_deref().or(zone.as_deref()),
                    )),
                    None if known.contains_key(href.as_str()) => changes.deleted.push(href),
                    None => {}
                }
            }
        }
        Ok(changes)
    }

    async fn put_object(
        &self,
        href: &str,
        payload: &str,
        precondition: &Precondition,
    ) -> ProviderResult<WriteResult> {
        let (calendar, existing_id, master_id) = split_href(href)?;
        let plan: Value = serde_json::from_str(payload).map_err(|_| invalid("bad write plan"))?;
        let uid = plan["uid"].as_str().unwrap_or(&master_id).to_owned();
        let conditional = !matches!(precondition, Precondition::Overwrite);
        for op in plan["ops"].as_array().into_iter().flatten() {
            let event_id = op["eventId"]
                .as_str()
                .unwrap_or(MASTER)
                .replace(MASTER, &master_id);
            let mut body = op["body"].clone();
            match op["method"].as_str() {
                Some("INSERT") => {
                    body["id"] = json!(master_id);
                    body["iCalUID"] = json!(uid);
                    let (status, _) = self
                        .send(
                            Method::POST,
                            self.events_url(&calendar)?.as_str(),
                            None,
                            Some(&body),
                            &[StatusCode::CONFLICT],
                        )
                        .await?;
                    // 409: it exists already (an earlier attempt arrived).
                    if status == StatusCode::CONFLICT && existing_id.is_none() {
                        return Err(ProviderError::PreconditionFailed);
                    }
                }
                Some("PATCH") => {
                    let etag = op["etag"].as_str().filter(|_| conditional);
                    self.send(
                        Method::PATCH,
                        &format!("{calendar}events/{event_id}"),
                        etag,
                        Some(&body),
                        &[],
                    )
                    .await?;
                }
                Some("DELETE") => {
                    let etag = op["etag"].as_str().filter(|_| conditional);
                    self.send(
                        Method::DELETE,
                        &format!("{calendar}events/{event_id}"),
                        etag,
                        None,
                        &[StatusCode::NOT_FOUND, StatusCode::GONE],
                    )
                    .await?;
                }
                _ => return Err(invalid("unknown write step")),
            }
        }
        let (members, zone) = self
            .fetch_series(&calendar, &master_id)
            .await?
            .ok_or_else(|| ProviderError::NotFound(href.to_owned()))?;
        let object = self.object(&calendar, &master_id, &members, zone.as_deref());
        Ok(WriteResult {
            href: object.href,
            etag: object.etag,
            data: Some(object.data),
        })
    }

    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        let (calendar, _, master_id) = split_href(href)?;
        Ok(self
            .fetch_series(&calendar, &master_id)
            .await?
            .map(|(members, zone)| self.object(&calendar, &master_id, &members, zone.as_deref())))
    }

    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()> {
        let (calendar, _, master_id) = split_href(href)?;
        let Some((members, _)) = self.fetch_series(&calendar, &master_id).await? else {
            return Ok(()); // already gone
        };
        if let Some(v) = version
            && series_etag(&members) != v
        {
            return Err(ProviderError::PreconditionFailed);
        }
        let master_etag = members
            .iter()
            .find(|e| !is_exception(e))
            .and_then(|e| e["etag"].as_str())
            .filter(|_| version.is_some());
        self.send(
            Method::DELETE,
            &format!("{calendar}events/{master_id}"),
            master_etag,
            None,
            &[StatusCode::NOT_FOUND, StatusCode::GONE],
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_base32hex() {
        let id = event_id_for("local-3f2a");
        assert!(id.len() >= 5);
        assert!(id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='v')));
        assert_eq!(id, event_id_for("local-3f2a"));
        let (cal, existing, master) =
            split_href("https://api/calendars/me%40x/local-3f2a.ics").unwrap();
        assert_eq!(
            (cal.as_str(), existing, master),
            ("https://api/calendars/me%40x/", None, id)
        );
        let (_, existing, master) = split_href("https://api/calendars/me%40x/events/abc").unwrap();
        assert_eq!((existing.as_deref(), master.as_str()), (Some("abc"), "abc"));
    }

    #[test]
    fn series_versions_ignore_member_order() {
        let a = json!({"etag": "\"1\""});
        let b = json!({"etag": "\"2\""});
        assert_eq!(
            series_etag(&[a.clone(), b.clone()]),
            series_etag(&[b, a.clone()])
        );
        assert_ne!(
            series_etag(std::slice::from_ref(&a)),
            series_etag(&[a, json!({"etag": "\"3\""})])
        );
    }
}
