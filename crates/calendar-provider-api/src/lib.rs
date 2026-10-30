//! Contract between the sync engine and calendar providers (CalDAV now;
//! Google and Microsoft later).
//!
//! Providers exchange *calendar object resources*: one iCalendar object per
//! remote item, identified by an `href` and versioned by an opaque `etag`.
//! CalDAV and ICS are natively shaped like this; JSON providers map their
//! events to iCalendar objects in their adapter so the engine stays uniform.
//! Writes are conditional on the version the client last saw.

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

/// Errors a provider can return, classified so the sync engine can decide
/// between retrying, backing off, re-authenticating and reporting.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderError {
    #[error("the server rejected the credentials")]
    Unauthorized,
    #[error("access denied: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The item changed on the server since it was last read (HTTP 412).
    #[error("the item was changed on the server")]
    PreconditionFailed,
    /// The incremental-sync cursor is no longer valid; do a full sync.
    #[error("the sync token is no longer valid")]
    SyncTokenInvalid,
    #[error("rate limited by the server")]
    RateLimited { retry_after: Option<Duration> },
    #[error("could not reach the server: {0}")]
    Network(String),
    #[error("server error (HTTP {status})")]
    Server { status: u16 },
    #[error("unexpected response from the server: {0}")]
    InvalidResponse(String),
}

impl ProviderError {
    /// Whether retrying later may succeed without user action.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::RateLimited { .. } | Self::Server { .. }
        )
    }
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// What a provider (or one of its calendars) supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Incremental sync via server-side cursors (e.g. RFC 6578 sync-token).
    pub incremental_sync: bool,
    /// Conditional writes with versions (ETag / If-Match).
    pub conditional_writes: bool,
}

/// A calendar collection on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCalendar {
    /// Server path of the collection, ending in `/`.
    pub href: String,
    pub name: String,
    /// `#rrggbb` if the server provides one.
    pub color: Option<String>,
    pub read_only: bool,
    /// Opaque collection version (CalDAV `getctag`), if offered.
    pub ctag: Option<String>,
    pub capabilities: Capabilities,
}

/// One calendar object resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteObject {
    pub href: String,
    pub etag: Option<String>,
    pub ical: String,
}

/// Result of asking a calendar for changes since a cursor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    /// Objects created or changed relative to what the client knows.
    pub upserted: Vec<RemoteObject>,
    /// Known hrefs that no longer exist on the server.
    pub deleted: Vec<String>,
    /// Cursor for the next call, if the provider supports incremental sync.
    pub cursor: Option<String>,
}

/// Result of a successful write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub href: String,
    /// New version, if the server returned one.
    pub etag: Option<String>,
}

/// How a write should be conditioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// The item must not exist yet (create).
    Create,
    /// The item must still have this version.
    Version(String),
    /// Replace whatever is there (the user chose to keep their version).
    Overwrite,
}

#[async_trait]
pub trait CalendarProvider: Send + Sync {
    /// Lists the calendars of the account.
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>>;

    /// Changes in `calendar` since `cursor` (`None` = full listing).
    /// `known` lists the versions the client holds, so providers without
    /// incremental sync can fetch only what changed.
    async fn fetch_changes(
        &self,
        calendar: &RemoteCalendar,
        cursor: Option<&str>,
        known: &[(String, Option<String>)],
    ) -> ProviderResult<ChangeSet>;

    /// Creates or replaces an object.
    async fn put_object(
        &self,
        href: &str,
        ical: &str,
        precondition: &Precondition,
    ) -> ProviderResult<WriteResult>;

    /// The current version of one object, or `None` if it does not exist.
    /// Used when the client does not know an object's version (e.g. the
    /// server answered a write without an ETag, RFC 4791 §5.3.4).
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>>;

    /// Deletes an object; `version` makes the delete conditional.
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()>;
}
