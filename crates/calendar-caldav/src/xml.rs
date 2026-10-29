//! WebDAV multistatus parsing (RFC 4918 §13) and request bodies.

use calendar_provider_api::ProviderError;

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CALSERVER: &str = "http://calendarserver.org/ns/";
pub const APPLE: &str = "http://apple.com/ns/ical/";

/// One property from a successful (2xx) propstat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub ns: String,
    pub name: String,
    /// Concatenated text content.
    pub text: String,
    /// `DAV:href` values found anywhere inside the property.
    pub hrefs: Vec<String>,
    /// Namespaced names of all descendant elements (e.g. resource types,
    /// privileges, supported components with their `name` attribute).
    pub elements: Vec<(String, String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub href: String,
    /// Response-level status (used e.g. for deleted members in sync reports).
    pub status: Option<u16>,
    pub props: Vec<Prop>,
}

impl Response {
    pub fn prop(&self, ns: &str, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.ns == ns && p.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Multistatus {
    pub responses: Vec<Response>,
    pub sync_token: Option<String>,
}

fn invalid(what: impl Into<String>) -> ProviderError {
    ProviderError::InvalidResponse(what.into())
}

fn status_code(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

fn is(node: &roxmltree::Node, ns: &str, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == Some(ns) && node.tag_name().name() == name
}

fn text_of(node: &roxmltree::Node) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn parse_multistatus(body: &str) -> Result<Multistatus, ProviderError> {
    let doc = roxmltree::Document::parse(body).map_err(|e| invalid(format!("XML: {e}")))?;
    let root = doc.root_element();
    if !is(&root, DAV, "multistatus") {
        return Err(invalid("expected DAV:multistatus"));
    }
    let mut out = Multistatus::default();
    for child in root.children().filter(|n| n.is_element()) {
        if is(&child, DAV, "sync-token") {
            out.sync_token = Some(text_of(&child));
            continue;
        }
        if !is(&child, DAV, "response") {
            continue;
        }
        let href = child
            .children()
            .find(|n| is(n, DAV, "href"))
            .map(|n| text_of(&n))
            .ok_or_else(|| invalid("response without href"))?;
        let status = child
            .children()
            .find(|n| is(n, DAV, "status"))
            .and_then(|n| status_code(&text_of(&n)));
        let mut props = Vec::new();
        for propstat in child.children().filter(|n| is(n, DAV, "propstat")) {
            let ok = propstat
                .children()
                .find(|n| is(n, DAV, "status"))
                .and_then(|n| status_code(&text_of(&n)))
                .is_some_and(|s| (200..300).contains(&s));
            if !ok {
                continue;
            }
            for prop in propstat.children().filter(|n| is(n, DAV, "prop")) {
                for p in prop.children().filter(|n| n.is_element()) {
                    let elements = p
                        .descendants()
                        .skip(1)
                        .filter(|n| n.is_element())
                        .map(|n| {
                            (
                                n.tag_name().namespace().unwrap_or_default().to_owned(),
                                n.tag_name().name().to_owned(),
                                n.attribute("name").map(str::to_owned),
                            )
                        })
                        .collect();
                    props.push(Prop {
                        ns: p.tag_name().namespace().unwrap_or_default().to_owned(),
                        name: p.tag_name().name().to_owned(),
                        text: text_of(&p),
                        hrefs: p
                            .descendants()
                            .filter(|n| is(n, DAV, "href"))
                            .map(|n| text_of(&n))
                            .collect(),
                        elements,
                    });
                }
            }
        }
        out.responses.push(Response {
            href,
            status,
            props,
        });
    }
    Ok(out)
}

/// Whether an error body says the sync token is invalid (RFC 6578 §3.2).
pub fn is_invalid_sync_token(body: &str) -> bool {
    roxmltree::Document::parse(body)
        .map(|doc| doc.descendants().any(|n| is(&n, DAV, "valid-sync-token")))
        .unwrap_or(false)
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub const PROPFIND_PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;

pub const PROPFIND_HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#;

pub const PROPFIND_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/">
<d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/><d:current-user-privilege-set/><cs:getctag/><d:sync-token/><d:supported-report-set/></d:prop>
</d:propfind>"#;

pub const PROPFIND_ETAGS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:getetag/><d:resourcetype/></d:prop></d:propfind>"#;

pub fn sync_collection(token: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
        escape(token)
    )
}

pub fn multiget(hrefs: &[String]) -> String {
    let mut body = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop>"#,
    );
    for h in hrefs {
        body.push_str("<d:href>");
        body.push_str(&escape(h));
        body.push_str("</d:href>");
    }
    body.push_str("</c:calendar-multiget>");
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_propstats_statuses_and_sync_tokens() {
        let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:x1="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/dav/calendars/alice/work/</d:href>
    <d:propstat><d:prop>
        <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
        <d:displayname>Work</d:displayname>
        <x1:calendar-color>#FF0000FF</x1:calendar-color>
        <cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/></cal:supported-calendar-component-set>
      </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
    <d:propstat><d:prop><d:getctag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
  </d:response>
  <d:response><d:href>/dav/calendars/alice/work/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
  <d:sync-token>http://example.com/sync/42</d:sync-token>
</d:multistatus>"#;
        let ms = parse_multistatus(body).unwrap();
        assert_eq!(ms.sync_token.as_deref(), Some("http://example.com/sync/42"));
        assert_eq!(ms.responses.len(), 2);
        let r = &ms.responses[0];
        assert_eq!(r.prop(DAV, "displayname").unwrap().text, "Work");
        assert!(
            r.prop(DAV, "resourcetype")
                .unwrap()
                .elements
                .iter()
                .any(|(ns, n, _)| ns == CALDAV && n == "calendar")
        );
        let comps = &r
            .prop(CALDAV, "supported-calendar-component-set")
            .unwrap()
            .elements;
        assert!(
            comps
                .iter()
                .any(|(_, _, name)| name.as_deref() == Some("VEVENT"))
        );
        assert!(
            r.prop(CALSERVER, "getctag").is_none(),
            "404 propstats are ignored"
        );
        assert_eq!(ms.responses[1].status, Some(404));
    }

    #[test]
    fn rejects_non_multistatus_and_detects_invalid_tokens() {
        assert!(parse_multistatus("<html/>").is_err());
        assert!(parse_multistatus("not xml").is_err());
        assert!(is_invalid_sync_token(
            r#"<d:error xmlns:d="DAV:"><d:valid-sync-token/></d:error>"#
        ));
        assert!(!is_invalid_sync_token("<d:error xmlns:d=\"DAV:\"/>"));
    }

    #[test]
    fn escapes_hrefs_in_bodies() {
        assert!(multiget(&["/a&b.ics".into()]).contains("<d:href>/a&amp;b.ics</d:href>"));
    }
}
