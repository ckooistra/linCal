//! Read-only ICS subscriptions: an iCalendar feed at a URL (`webcal://`,
//! `https://`), such as a public holiday calendar or the "secret address"
//! of a calendar in another service.
//!
//! * The feed is one calendar. It is split into one object per UID (a
//!   series with its overrides, or a single event), each wrapped in its own
//!   VCALENDAR with the time zones it uses, so the engine's usual object
//!   diff, deletion and import apply. Versions are content hashes.
//! * Every refresh is a conditional GET (ETag / Last-Modified), at most
//!   every [`MIN_REFRESH`].
//! * The URL may itself be secret (private feed addresses), so it is kept
//!   as the account's credential and never appears in hrefs or logs:
//!   objects are addressed as `ics:feed/<uid>`.
//! * Writes are refused: subscriptions are read-only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use calendar_caldav::ICalendarCodec;
use calendar_ical::{Component, Property};
use calendar_provider_api::{
    CalendarProvider, Capabilities, ChangeSet, ObjectCodec, Precondition, ProviderError,
    ProviderResult, RemoteCalendar, RemoteObject, WriteResult,
};
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use url::Url;

/// The calendar of a subscription (one per account).
pub const CALENDAR_HREF: &str = "ics:feed/";

/// Feeds are not fetched more often than this, even if the sync scheduler
/// runs more often.
pub const MIN_REFRESH: Duration = Duration::from_secs(15 * 60);

/// Largest feed accepted.
const MAX_FEED_BYTES: usize = 20 * 1024 * 1024;

/// The address to fetch for what the user entered: `webcal://` and
/// `webcals://` mean HTTPS. Plain HTTP is accepted for localhost only.
pub fn feed_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    let rest = input
        .strip_prefix("webcals://")
        .or_else(|| input.strip_prefix("webcal://"));
    let text = match rest {
        Some(rest) => format!("https://{rest}"),
        None if input.contains("://") => input.to_owned(),
        None => format!("https://{input}"),
    };
    let url = Url::parse(&text).map_err(|_| "enter the address of an iCalendar feed".to_owned())?;
    check_transport(&url)?;
    if url.host_str().is_none_or(str::is_empty) {
        return Err("enter the address of an iCalendar feed".into());
    }
    Ok(url)
}

fn check_transport(url: &Url) -> Result<(), String> {
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err("the feed address must use https:// or webcal:// \
                  (plain http is only allowed for localhost)"
            .into()),
    }
}

/// A feed split into objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    /// `X-WR-CALNAME`, if the feed names itself.
    pub name: Option<String>,
    /// `X-APPLE-CALENDAR-COLOR` as `#rrggbb`, if given.
    pub color: Option<String>,
    pub objects: Vec<RemoteObject>,
}

/// Splits feed text into one object per UID. Events without a UID are
/// identified by a hash of their content. Components other than events
/// (tasks, journals) are ignored.
pub fn split_feed(text: &str) -> Result<Feed, String> {
    let cal = calendar_ical::parse(text).map_err(|_| "not an iCalendar feed".to_owned())?;
    if cal.name != "VCALENDAR" {
        return Err("not an iCalendar feed".into());
    }
    let text_value = |name: &str| {
        cal.property(name)
            .map(|p| calendar_ical::unescape_text(&p.value).trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let zones: HashMap<&str, &Component> = cal
        .children("VTIMEZONE")
        .filter_map(|z| Some((z.property("TZID")?.value.as_str(), z)))
        .collect();
    let mut groups: BTreeMap<String, Vec<&Component>> = BTreeMap::new();
    for event in cal.children("VEVENT") {
        let uid = event
            .property("UID")
            .map(|p| p.value.trim().to_owned())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("no-uid-{:016x}", fnv(event.to_ics().as_bytes())));
        groups.entry(uid).or_default().push(event);
    }
    let mut objects = Vec::with_capacity(groups.len());
    for (uid, events) in groups {
        let mut object = Component::new("VCALENDAR");
        object.set(Property::new("VERSION", "2.0"));
        object.set(
            cal.property("PRODID")
                .cloned()
                .unwrap_or_else(|| Property::new("PRODID", "-//Universal Calendar//ICS//EN")),
        );
        let mut used: Vec<&str> = events
            .iter()
            .flat_map(|e| &e.properties)
            .flat_map(|p| &p.params)
            .filter(|p| p.name == "TZID")
            .flat_map(|p| p.values.iter().map(String::as_str))
            .collect();
        used.sort_unstable();
        used.dedup();
        for tzid in used {
            if let Some(zone) = zones.get(tzid) {
                object.components.push((*zone).clone());
            }
        }
        object.components.extend(events.into_iter().cloned());
        let data = object.to_ics();
        objects.push(RemoteObject {
            href: object_href(&uid),
            etag: Some(format!("{:016x}", fnv(data.as_bytes()))),
            data,
        });
    }
    Ok(Feed {
        name: text_value("X-WR-CALNAME"),
        color: text_value("X-APPLE-CALENDAR-COLOR").and_then(|c| color(&c)),
        objects,
    })
}

fn object_href(uid: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(uid.as_bytes()).collect();
    format!("{CALENDAR_HREF}{encoded}")
}

/// `#rrggbb` (or `#rrggbbaa`, alpha dropped) → `#rrggbb`.
fn color(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    (matches!(hex.len(), 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex[..6].to_ascii_lowercase()))
}

/// FNV-1a: a stable content hash for object versions.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What the last fetch returned, kept as the sync cursor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Validators {
    etag: Option<String>,
    modified: Option<String>,
    /// When the feed was last fetched (seconds since the epoch).
    at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub struct IcsProvider {
    http: reqwest::Client,
    url: Url,
    name: String,
    min_refresh: Duration,
}

impl IcsProvider {
    /// A provider for the feed at `url` (see [`feed_url`]), shown as `name`.
    pub fn new(url: Url, name: &str) -> ProviderResult<Self> {
        check_transport(&url).map_err(ProviderError::InvalidResponse)?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("UniversalCalendar/", env!("CARGO_PKG_VERSION")))
            // Redirects may not downgrade to plain HTTP.
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if check_transport(attempt.url()).is_err() {
                    attempt.error("redirect to an insecure address")
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok(Self {
            http,
            url,
            name: name.to_owned(),
            min_refresh: MIN_REFRESH,
        })
    }

    /// Changes the minimum time between downloads (tests).
    #[must_use]
    pub fn with_min_refresh(mut self, interval: Duration) -> Self {
        self.min_refresh = interval;
        self
    }

    /// Fetches and splits the feed (used when subscribing).
    pub async fn probe(&self) -> ProviderResult<Feed> {
        let (text, _) = self
            .download(&Validators::default())
            .await?
            .ok_or_else(|| ProviderError::InvalidResponse("the feed sent no content".into()))?;
        split_feed(&text).map_err(ProviderError::InvalidResponse)
    }

    /// The feed text and its new validators; `None` if unchanged (304).
    async fn download(
        &self,
        previous: &Validators,
    ) -> ProviderResult<Option<(String, Validators)>> {
        let mut request = self.http.get(self.url.clone());
        if let Some(etag) = &previous.etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &previous.modified {
            request = request.header(IF_MODIFIED_SINCE, modified);
        }
        let mut response = request.send().await.map_err(|e| {
            // Never include the (possibly secret) URL in messages.
            let e = e.without_url();
            if e.is_timeout() {
                ProviderError::Network("the feed did not answer in time".into())
            } else {
                ProviderError::Network(e.to_string())
            }
        })?;
        let status = response.status();
        if status == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        match status.as_u16() {
            200..=299 => {}
            401 => return Err(ProviderError::Unauthorized),
            403 => return Err(ProviderError::Forbidden("the feed refused access".into())),
            404 | 410 => {
                return Err(ProviderError::InvalidResponse(
                    "the feed no longer exists at this address".into(),
                ));
            }
            429 => return Err(ProviderError::RateLimited { retry_after: None }),
            code @ 500..=599 => return Err(ProviderError::Server { status: code }),
            code => {
                return Err(ProviderError::InvalidResponse(format!(
                    "the feed answered HTTP {code}"
                )));
            }
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let validators = Validators {
            etag: header(ETAG),
            modified: header(LAST_MODIFIED),
            at: now_secs(),
        };
        if response
            .content_length()
            .is_some_and(|n| n > MAX_FEED_BYTES as u64)
        {
            return Err(ProviderError::InvalidResponse(
                "the feed is too large".into(),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ProviderError::Network(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > MAX_FEED_BYTES {
                return Err(ProviderError::InvalidResponse(
                    "the feed is too large".into(),
                ));
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(body)
            .map_err(|_| ProviderError::InvalidResponse("the feed is not UTF-8 text".into()))?;
        Ok(Some((text, validators)))
    }
}

fn read_only() -> ProviderError {
    ProviderError::Forbidden("subscriptions are read-only".into())
}

#[async_trait]
impl CalendarProvider for IcsProvider {
    fn codec(&self) -> Arc<dyn ObjectCodec> {
        Arc::new(ICalendarCodec)
    }

    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>> {
        Ok(vec![RemoteCalendar {
            href: CALENDAR_HREF.into(),
            name: self.name.clone(),
            color: None,
            read_only: true,
            ctag: None,
            capabilities: Capabilities::default(),
        }])
    }

    async fn fetch_changes(
        &self,
        _calendar: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet> {
        let previous: Validators = cursor
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default();
        let unchanged = |validators: &Validators| ChangeSet {
            cursor: serde_json::to_string(validators).ok(),
            ..ChangeSet::default()
        };
        // A complete earlier fetch is recent enough.
        if cursor.is_some() && now_secs().saturating_sub(previous.at) < self.min_refresh.as_secs() {
            return Ok(unchanged(&previous));
        }
        // Validators only help when what they describe was fully read.
        let conditional = if cursor.is_some() {
            previous.clone()
        } else {
            Validators::default()
        };
        let Some((text, validators)) = self.download(&conditional).await? else {
            return Ok(unchanged(&Validators {
                at: now_secs(),
                ..previous
            }));
        };
        let feed = split_feed(&text).map_err(ProviderError::InvalidResponse)?;
        let known: HashMap<&str, Option<&str>> = known
            .iter()
            .map(|(href, etag)| (href.as_str(), etag.as_deref()))
            .collect();
        let listed: std::collections::HashSet<&str> =
            feed.objects.iter().map(|o| o.href.as_str()).collect();
        let deleted = known
            .keys()
            .filter(|href| !listed.contains(*href))
            .map(|href| (*href).to_owned())
            .collect();
        let upserted = feed
            .objects
            .iter()
            .filter(|o| known.get(o.href.as_str()) != Some(&o.etag.as_deref()))
            .cloned()
            .collect();
        Ok(ChangeSet {
            upserted,
            deleted,
            cursor: serde_json::to_string(&validators).ok(),
        })
    }

    async fn put_object(
        &self,
        _href: &str,
        _payload: &str,
        _precondition: &Precondition,
    ) -> ProviderResult<WriteResult> {
        Err(read_only())
    }

    async fn get_object(&self, _href: &str) -> ProviderResult<Option<RemoteObject>> {
        Err(read_only())
    }

    async fn delete_object(&self, _href: &str, _version: Option<&str>) -> ProviderResult<()> {
        Err(read_only())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Holidays//EN\r\n\
X-WR-CALNAME:Holidays\\, DE\r\nX-APPLE-CALENDAR-COLOR:#FF2968FF\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VTIMEZONE\r\nTZID:Unused/Zone\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:new-year\r\nDTSTART;VALUE=DATE:20270101\r\nSUMMARY:New Year\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:meet@x\r\nDTSTART;TZID=Europe/Berlin:20261102T090000\r\n\
DURATION:PT1H\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Meeting\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:meet@x\r\nRECURRENCE-ID;TZID=Europe/Berlin:20261109T090000\r\n\
DTSTART;TZID=Europe/Berlin:20261109T100000\r\nDURATION:PT1H\r\nSUMMARY:Meeting (late)\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20270501\r\nSUMMARY:No UID\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:task\r\nSUMMARY:Ignored\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    #[test]
    fn webcal_and_transport() {
        assert_eq!(
            feed_url(" webcal://example.org/a.ics ").unwrap().as_str(),
            "https://example.org/a.ics"
        );
        assert_eq!(
            feed_url("example.org/a.ics").unwrap().as_str(),
            "https://example.org/a.ics"
        );
        assert!(feed_url("http://example.org/a.ics").is_err());
        assert!(feed_url("http://localhost:8080/a.ics").is_ok());
        assert!(feed_url("ftp://example.org/a.ics").is_err());
        assert!(feed_url("").is_err());
    }

    #[test]
    fn splits_by_uid_with_the_zones_used() {
        let feed = split_feed(FEED).unwrap();
        assert_eq!(feed.name.as_deref(), Some("Holidays, DE"));
        assert_eq!(feed.color.as_deref(), Some("#ff2968"));
        let hrefs: Vec<&str> = feed.objects.iter().map(|o| o.href.as_str()).collect();
        assert_eq!(hrefs.len(), 3);
        assert!(hrefs.contains(&"ics:feed/meet%40x"));
        assert!(hrefs.contains(&"ics:feed/new-year"));
        let meeting = feed
            .objects
            .iter()
            .find(|o| o.href == "ics:feed/meet%40x")
            .unwrap();
        assert_eq!(meeting.data.matches("BEGIN:VEVENT").count(), 2);
        assert!(meeting.data.contains("TZID:Europe/Berlin"));
        assert!(!meeting.data.contains("Unused/Zone"));
        assert!(!meeting.data.contains("VTODO"));
        // Each object reads as an event series with its override.
        let read = ICalendarCodec.read(&meeting.data).unwrap();
        assert!(read.content.master.unwrap().recurrence.is_some());
        assert_eq!(read.content.overrides.len(), 1);
        // Versions are stable and change with the content.
        let again = split_feed(FEED).unwrap();
        assert_eq!(again.objects, feed.objects);
        let changed = split_feed(&FEED.replace("SUMMARY:New Year", "SUMMARY:Neujahr")).unwrap();
        let etag = |f: &Feed, h: &str| f.objects.iter().find(|o| o.href == h).unwrap().etag.clone();
        assert_ne!(
            etag(&changed, "ics:feed/new-year"),
            etag(&feed, "ics:feed/new-year")
        );
        assert_eq!(
            etag(&changed, "ics:feed/meet%40x"),
            etag(&feed, "ics:feed/meet%40x")
        );
        assert!(split_feed("<html>not a calendar</html>").is_err());
    }
}
