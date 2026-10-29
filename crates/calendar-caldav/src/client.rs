//! Minimal WebDAV/CalDAV HTTP client.

use std::time::Duration;

use calendar_provider_api::ProviderError;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use url::Url;

const MAX_REDIRECTS: usize = 5;

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    /// Final URL after redirects.
    pub url: Url,
}

/// HTTP client bound to one server origin and one set of credentials.
///
/// Credentials are only ever sent to the origin the user entered: redirects
/// to another host are refused rather than followed.
pub struct Client {
    http: reqwest::Client,
    username: String,
    password: String,
    origin: url::Origin,
}

fn network(e: reqwest::Error) -> ProviderError {
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    ProviderError::Network(kind.to_owned())
}

pub fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("valid method")
}

impl Client {
    pub fn new(server: &Url, username: &str, password: &str) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("UniversalCalendar/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ProviderError::Network(format!("HTTP client: {e}")))?;
        Ok(Self {
            http,
            username: username.to_owned(),
            password: password.to_owned(),
            origin: server.origin(),
        })
    }

    /// Sends a request, following same-origin redirects, and maps error
    /// statuses to [`ProviderError`]s. `ok` lists additional statuses the
    /// caller handles itself.
    pub async fn send(
        &self,
        method: Method,
        url: &Url,
        headers: &[(&str, String)],
        body: Option<String>,
        ok: &[StatusCode],
    ) -> Result<Reply, ProviderError> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            if url.origin() != self.origin {
                return Err(ProviderError::InvalidResponse(format!(
                    "the server redirected to {}; enter that address instead",
                    url.host_str().unwrap_or("another server")
                )));
            }
            let mut request = self
                .http
                .request(method.clone(), url.clone())
                .basic_auth(&self.username, Some(&self.password));
            for (name, value) in headers {
                request = request.header(
                    *name,
                    HeaderValue::from_str(value).map_err(|_| {
                        ProviderError::InvalidResponse("invalid header value".into())
                    })?,
                );
            }
            if let Some(b) = &body {
                request = request
                    .header(
                        "Content-Type",
                        if b.starts_with("BEGIN:") {
                            "text/calendar; charset=utf-8"
                        } else {
                            "application/xml; charset=utf-8"
                        },
                    )
                    .body(b.clone());
            }
            let response = request.send().await.map_err(network)?;
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get("Location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        ProviderError::InvalidResponse("redirect without Location".into())
                    })?;
                url = url
                    .join(location)
                    .map_err(|_| ProviderError::InvalidResponse("bad redirect".into()))?;
                continue;
            }
            let headers = response.headers().clone();
            let body = response.text().await.map_err(network)?;
            if status.is_success() || ok.contains(&status) {
                return Ok(Reply {
                    status,
                    headers,
                    body,
                    url,
                });
            }
            return Err(map_status(status, &headers, &url));
        }
        Err(ProviderError::InvalidResponse("too many redirects".into()))
    }
}

pub fn map_status(status: StatusCode, headers: &HeaderMap, url: &Url) -> ProviderError {
    match status.as_u16() {
        401 => ProviderError::Unauthorized,
        403 => ProviderError::Forbidden(url.path().to_owned()),
        404 | 410 => ProviderError::NotFound(url.path().to_owned()),
        412 => ProviderError::PreconditionFailed,
        429 | 503 => ProviderError::RateLimited {
            retry_after: headers
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
        },
        s if s >= 500 => ProviderError::Server { status: s },
        s => ProviderError::InvalidResponse(format!("HTTP {s}")),
    }
}
