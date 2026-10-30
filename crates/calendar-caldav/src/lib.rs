//! CalDAV provider (RFC 4791, RFC 6578, RFC 6764 discovery).
//!
//! Works with any compliant server; Nextcloud is the reference. Nothing is
//! server-specific: discovery starts from the URL the user enters and
//! follows `current-user-principal` → `calendar-home-set`, trying
//! `/.well-known/caldav` when the entered URL is not a DAV endpoint.

mod client;
pub mod xml;

use std::collections::HashMap;

use async_trait::async_trait;
use calendar_core::{ObjectContent, OccurrenceKey};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, ObjectCodec, Precondition, ProviderError,
    ProviderResult, ReadObject, RemoteCalendar, RemoteObject, WriteResult,
};
use reqwest::{Method, StatusCode};
use std::sync::Arc;
use url::Url;

use client::{Client, method};
use xml::{APPLE, CALDAV, CALSERVER, DAV, Multistatus, parse_multistatus};

/// Objects fetched per calendar-multiget request.
const MULTIGET_BATCH: usize = 50;

/// CalDAV objects are iCalendar text: read and written with `calendar-ical`,
/// editing the stored server copy in place.
pub struct ICalendarCodec;

impl ObjectCodec for ICalendarCodec {
    fn read(&self, data: &str) -> Result<ReadObject, String> {
        let cal = calendar_ical::parse(data).map_err(|e| e.to_string())?;
        let mut events = calendar_ical::read_events(&cal).map_err(|e| e.to_string())?;
        let warnings = std::mem::take(&mut events.warnings);
        Ok(ReadObject {
            content: events.into_content(),
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
        let base = base
            .filter(|b| !b.trim().is_empty())
            .and_then(|b| calendar_ical::parse(b).ok());
        calendar_ical::write_object(
            base.as_ref(),
            uid,
            content.master.as_ref(),
            &content.overrides,
            renamed,
            chrono::Utc::now(),
        )
        .to_ics()
    }
}

/// Result of account discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub principal: Url,
    pub calendar_home: Url,
}

pub struct CalDavProvider {
    client: Client,
    calendar_home: Url,
}

fn resolve(base: &Url, href: &str) -> ProviderResult<Url> {
    base.join(href)
        .map_err(|_| ProviderError::InvalidResponse(format!("bad href {href:?}")))
}

fn same_resource(a: &Url, b: &Url) -> bool {
    a.as_str().trim_end_matches('/') == b.as_str().trim_end_matches('/')
}

async fn propfind(
    client: &Client,
    url: &Url,
    depth: &str,
    body: &str,
) -> ProviderResult<(Url, Multistatus)> {
    let reply = client
        .send(
            method("PROPFIND"),
            url,
            &[("Depth", depth.to_owned())],
            Some(body.to_owned()),
            &[],
        )
        .await?;
    if reply.status != StatusCode::MULTI_STATUS {
        return Err(ProviderError::InvalidResponse(format!(
            "expected 207 Multi-Status, got {}",
            reply.status
        )));
    }
    Ok((reply.url, parse_multistatus(&reply.body)?))
}

async fn find_href(
    client: &Client,
    url: &Url,
    body: &str,
    ns: &str,
    name: &str,
) -> ProviderResult<Option<Url>> {
    let (base, ms) = propfind(client, url, "0", body).await?;
    for r in &ms.responses {
        if let Some(href) = r.prop(ns, name).and_then(|p| p.hrefs.first()) {
            return Ok(Some(resolve(&base, href)?));
        }
    }
    Ok(None)
}

/// Whether credentials for `server` may be sent to `target`, a URL the
/// server reported during discovery: the same origin, or (over HTTPS, never
/// downgraded) another host of the same domain, as some providers keep
/// calendars on per-user hosts (`p07-caldav.example.com`).
fn trusted_transition(server: &Url, target: &Url) -> bool {
    if server.origin() == target.origin() {
        return true;
    }
    let site = |u: &Url| -> Option<String> {
        let host = u.host_str()?.to_ascii_lowercase();
        let labels: Vec<&str> = host.split('.').collect();
        (labels.len() >= 2).then(|| labels[labels.len() - 2..].join("."))
    };
    target.scheme() == "https"
        && site(server).is_some()
        && site(server) == site(target)
        && server
            .host_str()
            .is_some_and(|h| h.parse::<std::net::IpAddr>().is_err())
}

/// Finds the principal and calendar home for `server` with these credentials.
///
/// The calendar home must be on a trusted origin (see `trusted_transition`):
/// account setup later sends the password there.
pub async fn discover(server: &Url, username: &str, password: &str) -> ProviderResult<Discovery> {
    let client = Client::new(server, username, password)?;
    let mut candidates = vec![server.clone()];
    if let Ok(well_known) = server.join("/.well-known/caldav") {
        candidates.push(well_known);
    }
    let mut last_error = None;
    for start in candidates {
        let principal = match find_href(
            &client,
            &start,
            xml::PROPFIND_PRINCIPAL,
            DAV,
            "current-user-principal",
        )
        .await
        {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(e @ ProviderError::Unauthorized) => return Err(e),
            Err(e) => {
                last_error = Some(e);
                continue;
            }
        };
        let home = find_href(
            &client,
            &principal,
            xml::PROPFIND_HOME,
            CALDAV,
            "calendar-home-set",
        )
        .await?
        .ok_or_else(|| {
            ProviderError::InvalidResponse("the server did not report a calendar home".into())
        })?;
        if !trusted_transition(server, &home) {
            return Err(ProviderError::InvalidResponse(format!(
                "the server keeps calendars at {}, which is not part of {}; enter that address instead",
                home.host_str().unwrap_or("another server"),
                server.host_str().unwrap_or("the server")
            )));
        }
        client::check_transport(&home)?;
        return Ok(Discovery {
            principal,
            calendar_home: home,
        });
    }
    Err(last_error.unwrap_or_else(|| {
        ProviderError::NotFound("no CalDAV service found at this address".into())
    }))
}

/// Converts `#RRGGBB` or `#RRGGBBAA` (Apple/Nextcloud) to `#rrggbb`.
fn normalize_color(value: &str) -> Option<String> {
    let hex = value.trim().strip_prefix('#')?;
    if (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex[..6].to_ascii_lowercase()))
    } else {
        None
    }
}

impl CalDavProvider {
    pub fn new(calendar_home: Url, username: &str, password: &str) -> ProviderResult<Self> {
        Ok(Self {
            client: Client::new(&calendar_home, username, password)?,
            calendar_home,
        })
    }

    async fn report(
        &self,
        url: &Url,
        depth: &str,
        body: String,
    ) -> ProviderResult<(Url, Multistatus)> {
        let reply = self
            .client
            .send(
                method("REPORT"),
                url,
                &[("Depth", depth.to_owned())],
                Some(body),
                &[StatusCode::FORBIDDEN, StatusCode::CONFLICT],
            )
            .await?;
        if reply.status == StatusCode::FORBIDDEN || reply.status == StatusCode::CONFLICT {
            if xml::is_invalid_sync_token(&reply.body) {
                return Err(ProviderError::SyncTokenInvalid);
            }
            return Err(client::map_status(reply.status, &reply.headers, &reply.url));
        }
        Ok((reply.url, parse_multistatus(&reply.body)?))
    }

    /// Member hrefs with ETags, via RFC 6578 sync-collection. Returns
    /// (changed, deleted, new token). An empty token lists everything.
    async fn sync_collection(
        &self,
        calendar: &Url,
        token: &str,
    ) -> ProviderResult<(Vec<(String, Option<String>)>, Vec<String>, Option<String>)> {
        let mut changed = Vec::new();
        let mut deleted = Vec::new();
        let mut token = token.to_owned();
        // Servers may truncate (507 on the collection) and expect a follow-up.
        for _ in 0..100 {
            let (base, ms) = self
                .report(calendar, "1", xml::sync_collection(&token))
                .await?;
            let mut truncated = false;
            for r in &ms.responses {
                let url = resolve(&base, &r.href)?;
                if same_resource(&url, calendar) {
                    truncated |= r.status == Some(507);
                    continue;
                }
                match r.status {
                    Some(404 | 410) => deleted.push(url.to_string()),
                    _ => changed.push((
                        url.to_string(),
                        r.prop(DAV, "getetag").map(|p| p.text.clone()),
                    )),
                }
            }
            let next = ms.sync_token.clone();
            if !truncated || next.as_deref() == Some(token.as_str()) || next.is_none() {
                return Ok((changed, deleted, next));
            }
            token = next.unwrap_or_default();
        }
        Err(ProviderError::InvalidResponse(
            "sync-collection did not finish".into(),
        ))
    }

    /// All member hrefs with ETags, via PROPFIND (fallback).
    async fn list_etags(&self, calendar: &Url) -> ProviderResult<Vec<(String, Option<String>)>> {
        let (base, ms) = propfind(&self.client, calendar, "1", xml::PROPFIND_ETAGS).await?;
        let mut out = Vec::new();
        for r in &ms.responses {
            let url = resolve(&base, &r.href)?;
            let is_collection = r
                .prop(DAV, "resourcetype")
                .is_some_and(|p| p.elements.iter().any(|(_, n, _)| n == "collection"));
            if same_resource(&url, calendar) || is_collection {
                continue;
            }
            out.push((
                url.to_string(),
                r.prop(DAV, "getetag").map(|p| p.text.clone()),
            ));
        }
        Ok(out)
    }

    async fn multiget(
        &self,
        calendar: &Url,
        hrefs: &[String],
    ) -> ProviderResult<Vec<RemoteObject>> {
        let mut out = Vec::with_capacity(hrefs.len());
        for chunk in hrefs.chunks(MULTIGET_BATCH) {
            let paths: Vec<String> = chunk
                .iter()
                .map(|h| {
                    Url::parse(h)
                        .map(|u| u.path().to_owned())
                        .unwrap_or_else(|_| h.clone())
                })
                .collect();
            let (base, ms) = self.report(calendar, "1", xml::multiget(&paths)).await?;
            for r in &ms.responses {
                let Some(data) = r.prop(CALDAV, "calendar-data") else {
                    continue; // 404 for items deleted meanwhile
                };
                out.push(RemoteObject {
                    href: resolve(&base, &r.href)?.to_string(),
                    etag: r.prop(DAV, "getetag").map(|p| p.text.clone()),
                    data: data.text.clone(),
                });
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl CalendarProvider for CalDavProvider {
    fn codec(&self) -> Arc<dyn ObjectCodec> {
        Arc::new(ICalendarCodec)
    }

    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        let (base, ms) = propfind(
            &self.client,
            &self.calendar_home,
            "1",
            xml::PROPFIND_CALENDARS,
        )
        .await?;
        let mut calendars = Vec::new();
        for r in &ms.responses {
            let is_calendar = r.prop(DAV, "resourcetype").is_some_and(|p| {
                p.elements
                    .iter()
                    .any(|(ns, n, _)| ns == CALDAV && n == "calendar")
            });
            if !is_calendar {
                continue;
            }
            // Skip collections that cannot hold events (task lists etc.).
            if let Some(comps) = r.prop(CALDAV, "supported-calendar-component-set")
                && !comps
                    .elements
                    .iter()
                    .any(|(_, _, name)| name.as_deref() == Some("VEVENT"))
            {
                continue;
            }
            let read_only = r.prop(DAV, "current-user-privilege-set").is_some_and(|p| {
                !p.elements.iter().any(|(ns, n, _)| {
                    ns == DAV && matches!(n.as_str(), "write" | "write-content" | "all")
                })
            });
            let url = resolve(&base, &r.href)?;
            let name = r
                .prop(DAV, "displayname")
                .map(|p| p.text.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    url.path_segments()
                        .and_then(|mut s| s.rfind(|seg| !seg.is_empty()).map(str::to_owned))
                        .unwrap_or_else(|| "Calendar".into())
                });
            let supports_sync = r.prop(DAV, "sync-token").is_some()
                || r.prop(DAV, "supported-report-set")
                    .is_some_and(|p| p.elements.iter().any(|(_, n, _)| n == "sync-collection"));
            calendars.push(RemoteCalendar {
                href: url.to_string(),
                name,
                color: r
                    .prop(APPLE, "calendar-color")
                    .and_then(|p| normalize_color(&p.text)),
                read_only,
                ctag: r.prop(CALSERVER, "getctag").map(|p| p.text.clone()),
                capabilities: Capabilities {
                    incremental_sync: supports_sync,
                    conditional_writes: true,
                },
            });
        }
        Ok(calendars)
    }

    async fn fetch_changes(
        &self,
        calendar: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        let url = Url::parse(&calendar.href)
            .map_err(|_| ProviderError::InvalidResponse("bad calendar URL".into()))?;
        let known: HashMap<&str, Option<&str>> = known
            .iter()
            .map(|(h, e)| (h.as_str(), e.as_deref()))
            .collect();

        let incremental = if calendar.capabilities.incremental_sync {
            match self.sync_collection(&url, cursor.unwrap_or("")).await {
                Ok(result) => Some((result, cursor.is_some())),
                // Expired token: fall back to a full listing below.
                Err(ProviderError::SyncTokenInvalid) => {
                    match self.sync_collection(&url, "").await {
                        Ok(result) => Some((result, false)),
                        Err(ProviderError::SyncTokenInvalid) => None,
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            None
        };

        let (listing, mut deleted, next, is_delta) = match incremental {
            Some(((changed, deleted, token), is_delta)) => (changed, deleted, token, is_delta),
            None => (self.list_etags(&url).await?, Vec::new(), None, false),
        };
        if !is_delta {
            // Complete listing: anything we know that was not listed is gone.
            let listed: std::collections::HashSet<&str> =
                listing.iter().map(|(h, _)| h.as_str()).collect();
            deleted.extend(
                known
                    .keys()
                    .filter(|h| !listed.contains(**h))
                    .map(|h| (*h).to_owned()),
            );
        }
        let to_fetch: Vec<String> = listing
            .into_iter()
            .filter(|(href, etag)| {
                etag.is_none() || known.get(href.as_str()).copied().flatten() != etag.as_deref()
            })
            .map(|(href, _)| href)
            .collect();
        let upserted = if to_fetch.is_empty() {
            Vec::new()
        } else {
            self.multiget(&url, &to_fetch).await?
        };
        Ok(ChangeSet {
            upserted,
            deleted,
            cursor: next,
        })
    }

    async fn put_object(
        &self,
        href: &str,
        payload: &str,
        precondition: &Precondition,
    ) -> ProviderResult<WriteResult> {
        let url = Url::parse(href)
            .map_err(|_| ProviderError::InvalidResponse("bad object URL".into()))?;
        let headers: Vec<(&str, String)> = match precondition {
            Precondition::Create => vec![("If-None-Match", "*".to_owned())],
            Precondition::Version(etag) => vec![("If-Match", etag.clone())],
            Precondition::Overwrite => vec![],
        };
        let reply = self
            .client
            .send(method("PUT"), &url, &headers, Some(payload.to_owned()), &[])
            .await?;
        let etag = reply
            .headers
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        // With an ETag the server stored the payload as sent (RFC 4791
        // §5.3.4); without one the engine reads the object back.
        Ok(WriteResult {
            href: url.to_string(),
            data: etag.is_some().then(|| payload.to_owned()),
            etag,
        })
    }

    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>> {
        let url = Url::parse(href)
            .map_err(|_| ProviderError::InvalidResponse("bad object URL".into()))?;
        match self.client.send(Method::GET, &url, &[], None, &[]).await {
            Ok(reply) => Ok(Some(RemoteObject {
                href: url.to_string(),
                etag: reply
                    .headers
                    .get("ETag")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned),
                data: reply.body,
            })),
            Err(ProviderError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()> {
        let url = Url::parse(href)
            .map_err(|_| ProviderError::InvalidResponse("bad object URL".into()))?;
        let headers: Vec<(&str, String)> = version
            .map(|v| ("If-Match", v.to_owned()))
            .into_iter()
            .collect();
        match self
            .client
            .send(method("DELETE"), &url, &headers, None, &[])
            .await
        {
            // Already gone is fine.
            Ok(_) | Err(ProviderError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_color;

    #[test]
    fn discovery_only_trusts_the_same_site() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        let server = u("https://cloud.example.org/remote.php/dav/");
        assert!(super::trusted_transition(
            &server,
            &u("https://cloud.example.org/cal/")
        ));
        assert!(super::trusted_transition(
            &server,
            &u("https://p07.example.org/cal/")
        ));
        assert!(!super::trusted_transition(
            &server,
            &u("http://cloud.example.org/cal/")
        ));
        assert!(!super::trusted_transition(
            &server,
            &u("https://evil.example.net/cal/")
        ));
        let ip = u("https://192.168.1.5/dav/");
        assert!(!super::trusted_transition(&ip, &u("https://10.1.5/dav/")));
        assert!(super::client::check_transport(&u("http://127.0.0.1:5232/")).is_ok());
        assert!(super::client::check_transport(&u("http://localhost/")).is_ok());
        assert!(super::client::check_transport(&u("http://cloud.example.org/")).is_err());
    }

    #[test]
    fn normalizes_colors() {
        assert_eq!(normalize_color("#FF0000FF").as_deref(), Some("#ff0000"));
        assert_eq!(normalize_color("#00aa11").as_deref(), Some("#00aa11"));
        assert_eq!(normalize_color("red"), None);
    }
}
