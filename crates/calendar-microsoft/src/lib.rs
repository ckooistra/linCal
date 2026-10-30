//! Microsoft Graph calendar provider (personal and work/school accounts).
//!
//! * Sync uses `calendarView/delta`, which is bound to a date window. The
//!   opaque cursor stores the window and the delta link; when the window
//!   needs to move forward, a full listing of the new window replaces it.
//! * Objects are instances (single events, occurrences, exceptions); see
//!   [`codec`] and `docs/decisions/0005-google-and-microsoft.md`.
//! * hrefs: calendar `…/me/calendars/<id>/`, object `…/me/calendars/<id>/events/<event id>`.
//!   Graph assigns ids on create; creates send `transactionId` so a retry
//!   does not duplicate, and report the new href.

pub mod codec;
#[cfg(feature = "testing")]
pub mod testing;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use calendar_oauth::{OAuthClient, OAuthError, TokenSource};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, ObjectCodec, Precondition, ProviderError,
    ProviderResult, RemoteCalendar, RemoteObject, WriteResult,
};
use chrono::{Datelike, NaiveDate, Utc};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

pub use codec::GraphCodec;

pub const API_BASE: &str = "https://graph.microsoft.com/v1.0/";
pub const SCOPES: &[&str] = &["offline_access", "User.Read", "Calendars.ReadWrite"];

/// Days of the past and future mirrored from Microsoft calendars.
const WINDOW_PAST_DAYS: i64 = 365;
const WINDOW_FUTURE_DAYS: i64 = 2 * 365;
/// The window moves forward once less than this much future is left.
const WINDOW_MIN_FUTURE_DAYS: i64 = 365;

/// OAuth settings for a Microsoft identity platform public client
/// ("Mobile and desktop applications" with redirect `http://localhost`).
pub fn oauth_client(client_id: &str) -> OAuthClient {
    OAuthClient {
        auth_url: Url::parse("https://login.microsoftonline.com/common/oauth2/v2.0/authorize")
            .expect("valid"),
        token_url: Url::parse("https://login.microsoftonline.com/common/oauth2/v2.0/token")
            .expect("valid"),
        client_id: client_id.to_owned(),
        client_secret: None,
        scopes: SCOPES.iter().map(|s| (*s).to_owned()).collect(),
        extra_params: vec![("prompt".into(), "select_account".into())],
        redirect_host: "localhost".into(),
    }
}

/// What a Microsoft account keeps in the OS keyring.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSecret {
    pub tokens: calendar_oauth::Tokens,
}

/// The sync cursor of one calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cursor {
    start: NaiveDate,
    end: NaiveDate,
    delta_link: String,
}

fn window(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    // Month-aligned, so the window (and the cursor) only changes monthly.
    let first = |d: NaiveDate| d.with_day(1).expect("day 1 exists");
    (
        first(today - chrono::Duration::days(WINDOW_PAST_DAYS)),
        first(today + chrono::Duration::days(WINDOW_FUTURE_DAYS)),
    )
}

pub struct GraphProvider {
    http: reqwest::Client,
    tokens: Arc<TokenSource>,
    api: Url,
    today: fn() -> NaiveDate,
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

fn etag_of(e: &Value) -> Option<String> {
    e["@odata.etag"].as_str().map(str::to_owned)
}

/// (calendar href, event id or `None` for an object not created yet).
fn split_href(href: &str) -> ProviderResult<(String, Option<String>)> {
    if let Some((calendar, id)) = href.split_once("/events/") {
        return Ok((format!("{calendar}/"), Some(id.to_owned())));
    }
    let (calendar, _) = href.rsplit_once('/').ok_or_else(|| invalid("bad href"))?;
    Ok((format!("{calendar}/"), None))
}

fn segment(href: &str) -> &str {
    href.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(href)
}

impl GraphProvider {
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
            today: || Utc::now().date_naive(),
        })
    }

    /// A provider for an account from its client id and stored secret.
    pub fn connect(client_id: &str, secret: &str, api: Url) -> ProviderResult<Self> {
        let secret: GraphSecret =
            serde_json::from_str(secret).map_err(|_| ProviderError::Unauthorized)?;
        Self::new(
            Arc::new(TokenSource::new(oauth_client(client_id), secret.tokens)),
            api,
        )
    }

    /// Replaces the clock (tests).
    pub fn with_today(mut self, today: fn() -> NaiveDate) -> Self {
        self.today = today;
        self
    }

    fn url(&self, path: &str) -> ProviderResult<Url> {
        self.api.join(path).map_err(|_| invalid("bad API URL"))
    }

    fn event_url(&self, id: &str) -> ProviderResult<Url> {
        let mut url = self.url("me/events")?;
        url.path_segments_mut()
            .map_err(|_| invalid("bad API URL"))?
            .push(id);
        Ok(url)
    }

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
                .header("Accept", "application/json")
                .header("Prefer", "outlook.timezone=\"UTC\", odata.maxpagesize=100");
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
            let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
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
            let code = json["error"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            return Err(match status.as_u16() {
                401 => ProviderError::Unauthorized,
                403 => ProviderError::Forbidden(code),
                404 | 410 => ProviderError::NotFound(url.to_owned()),
                409 | 412 => ProviderError::PreconditionFailed,
                429 | 503 => ProviderError::RateLimited { retry_after },
                s if s >= 500 => ProviderError::Server { status: s },
                s => ProviderError::InvalidResponse(format!("HTTP {s} {code}")),
            });
        }
        Err(ProviderError::Unauthorized)
    }

    /// Follows `@odata.nextLink` pages; returns the final `@odata.deltaLink`.
    async fn pages(
        &self,
        first: String,
        mut on_page: impl FnMut(&Value),
        ok: &[StatusCode],
    ) -> ProviderResult<Result<Option<String>, StatusCode>> {
        let mut next = first;
        for _ in 0..10_000 {
            let (status, body) = self.send(Method::GET, &next, None, None, ok).await?;
            if !status.is_success() {
                return Ok(Err(status));
            }
            on_page(&body);
            match body["@odata.nextLink"].as_str() {
                Some(link) => next = link.to_owned(),
                None => return Ok(Ok(body["@odata.deltaLink"].as_str().map(str::to_owned))),
            }
        }
        Err(invalid("too many pages"))
    }

    fn object(&self, calendar_href: &str, e: &Value) -> Option<RemoteObject> {
        let id = e["id"].as_str()?;
        Some(RemoteObject {
            href: format!("{calendar_href}events/{id}"),
            etag: etag_of(e),
            data: e.to_string(),
        })
    }

    /// The signed-in user's e-mail address.
    pub async fn account_email(&self) -> ProviderResult<String> {
        let (_, me) = self
            .send(Method::GET, self.url("me")?.as_str(), None, None, &[])
            .await?;
        me["mail"]
            .as_str()
            .or(me["userPrincipalName"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| invalid("account without an address"))
    }

    fn delta_url(
        &self,
        calendar_href: &str,
        start: NaiveDate,
        end: NaiveDate,
    ) -> ProviderResult<Url> {
        let mut url = self.url(&format!(
            "me/calendars/{}/calendarView/delta",
            segment(calendar_href)
        ))?;
        url.query_pairs_mut()
            .append_pair("startDateTime", &format!("{start}T00:00:00Z"))
            .append_pair("endDateTime", &format!("{end}T00:00:00Z"));
        Ok(url)
    }
}

#[async_trait]
impl CalendarProvider for GraphProvider {
    fn codec(&self) -> Arc<dyn ObjectCodec> {
        Arc::new(GraphCodec)
    }

    fn take_updated_secret(&self) -> Option<String> {
        let tokens = self.tokens.take_changed()?;
        serde_json::to_string(&GraphSecret { tokens }).ok()
    }

    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        let mut out = Vec::new();
        let base = self.url("me/calendars/")?;
        self.pages(
            self.url("me/calendars")?.to_string(),
            |page| {
                for c in page["value"].as_array().into_iter().flatten() {
                    let Some(id) = c["id"].as_str() else { continue };
                    let mut href = base.clone();
                    if let Ok(mut segments) = href.path_segments_mut() {
                        segments.pop_if_empty().extend([id, ""]);
                    }
                    out.push(RemoteCalendar {
                        href: href.to_string(),
                        name: c["name"].as_str().unwrap_or("Calendar").to_owned(),
                        color: c["hexColor"]
                            .as_str()
                            .filter(|v| v.len() == 7 && v.starts_with('#'))
                            .map(str::to_ascii_lowercase),
                        read_only: c["canEdit"].as_bool() == Some(false),
                        ctag: c["changeKey"].as_str().map(str::to_owned),
                        capabilities: Capabilities {
                            incremental_sync: true,
                            conditional_writes: true,
                        },
                    });
                }
            },
            &[],
        )
        .await?
        .map_err(|s| invalid(&format!("HTTP {s}")))?;
        Ok(out)
    }

    async fn fetch_changes(
        &self,
        calendar: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        let today = (self.today)();
        let stored: Option<Cursor> = cursor.and_then(|c| serde_json::from_str(c).ok());
        // Keep the window until too little future is left in it.
        let current = stored
            .filter(|c| (c.end - today).num_days() >= WINDOW_MIN_FUTURE_DAYS && c.start <= today);
        let mut items: Vec<Value> = Vec::new();
        let (start, end, delta_link, incremental) = match current {
            Some(c) => {
                let mut link_items = Vec::new();
                match self
                    .pages(
                        c.delta_link.clone(),
                        |page| {
                            link_items
                                .extend(page["value"].as_array().into_iter().flatten().cloned())
                        },
                        &[StatusCode::GONE],
                    )
                    .await?
                {
                    Ok(link) => {
                        items = link_items;
                        (c.start, c.end, link, true)
                    }
                    // The delta state expired: list the window again.
                    Err(_) => (c.start, c.end, None, false),
                }
            }
            None => {
                let (s, e) = window(today);
                (s, e, None, false)
            }
        };
        let delta_link = if incremental {
            delta_link
        } else {
            let mut full = Vec::new();
            let link = self
                .pages(
                    self.delta_url(&calendar.href, start, end)?.to_string(),
                    |page| full.extend(page["value"].as_array().into_iter().flatten().cloned()),
                    &[],
                )
                .await?
                .map_err(|s| invalid(&format!("HTTP {s}")))?;
            items = full;
            link
        };

        let mut changes = ChangeSet::default();
        let mut listed = HashSet::new();
        let known_map: std::collections::HashMap<&str, Option<&str>> = known
            .iter()
            .map(|(h, e)| (h.as_str(), e.as_deref()))
            .collect();
        for e in &items {
            let Some(id) = e["id"].as_str() else { continue };
            let href = format!("{}events/{id}", calendar.href);
            if e.get("@removed").is_some() || e["isCancelled"].as_bool() == Some(true) {
                if known_map.contains_key(href.as_str()) {
                    changes.deleted.push(href);
                }
                continue;
            }
            // Series masters carry no times of their own here; their
            // occurrences are listed individually.
            if e["type"].as_str() == Some("seriesMaster") {
                continue;
            }
            listed.insert(href.clone());
            if let Some(object) = self.object(&calendar.href, e)
                && known_map.get(href.as_str()).copied().flatten() != object.etag.as_deref()
            {
                changes.upserted.push(object);
            }
        }
        if !incremental {
            changes.deleted.extend(
                known_map
                    .keys()
                    .filter(|h| !listed.contains(**h))
                    .map(|h| (*h).to_owned()),
            );
        }
        changes.cursor = delta_link
            .map(|link| {
                serde_json::to_string(&Cursor {
                    start,
                    end,
                    delta_link: link,
                })
            })
            .transpose()
            .map_err(|e| invalid(&e.to_string()))?;
        Ok(changes)
    }

    async fn put_object(
        &self,
        href: &str,
        payload: &str,
        precondition: &Precondition,
    ) -> ProviderResult<WriteResult> {
        let (calendar, id) = split_href(href)?;
        let plan: Value = serde_json::from_str(payload).map_err(|_| invalid("bad write plan"))?;
        let body = &plan["body"];
        let (_, event) = match (plan["method"].as_str(), &id) {
            (Some("POST"), None) => {
                let url = self.url(&format!("me/calendars/{}/events", segment(&calendar)))?;
                self.send(Method::POST, url.as_str(), None, Some(body), &[])
                    .await?
            }
            (Some("PATCH" | "POST"), Some(id)) => {
                let etag = match precondition {
                    Precondition::Overwrite => None,
                    _ => plan["etag"].as_str(),
                };
                let url = self.event_url(id)?;
                if body.as_object().is_some_and(|b| b.is_empty()) {
                    self.send(Method::GET, url.as_str(), None, None, &[])
                        .await?
                } else {
                    let mut patch = body.clone();
                    if let Some(map) = patch.as_object_mut() {
                        map.remove("transactionId");
                    }
                    self.send(Method::PATCH, url.as_str(), etag, Some(&patch), &[])
                        .await?
                }
            }
            _ => return Err(invalid("unsupported write")),
        };
        let object = self
            .object(&calendar, &event)
            .ok_or_else(|| invalid("event without an id"))?;
        Ok(WriteResult {
            href: object.href,
            etag: object.etag,
            data: Some(object.data),
        })
    }

    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        let (calendar, Some(id)) = split_href(href)? else {
            return Ok(None);
        };
        let (status, event) = self
            .send(
                Method::GET,
                self.event_url(&id)?.as_str(),
                None,
                None,
                &[StatusCode::NOT_FOUND],
            )
            .await?;
        Ok(
            if status.is_success() && event["isCancelled"].as_bool() != Some(true) {
                self.object(&calendar, &event)
            } else {
                None
            },
        )
    }

    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()> {
        let (_, Some(id)) = split_href(href)? else {
            return Ok(());
        };
        self.send(
            Method::DELETE,
            self.event_url(&id)?.as_str(),
            version,
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
    fn windows_are_month_aligned() {
        let (s, e) = window(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        assert_eq!(s, NaiveDate::from_ymd_opt(2025, 10, 1).unwrap());
        assert_eq!(e, NaiveDate::from_ymd_opt(2028, 10, 1).unwrap());
    }

    #[test]
    fn hrefs() {
        let (cal, id) = split_href("https://g/v1.0/me/calendars/AAA=/events/BBB").unwrap();
        assert_eq!(
            (cal.as_str(), id.as_deref()),
            ("https://g/v1.0/me/calendars/AAA=/", Some("BBB"))
        );
        let (cal, id) = split_href("https://g/v1.0/me/calendars/AAA=/local-1.ics").unwrap();
        assert_eq!(
            (cal.as_str(), id),
            ("https://g/v1.0/me/calendars/AAA=/", None)
        );
        assert_eq!(segment("https://g/v1.0/me/calendars/AAA=/"), "AAA=");
    }
}
