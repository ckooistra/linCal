//! Contract between the sync engine and calendar providers (CalDAV, Google
//! Calendar, Microsoft Graph).
//!
//! Providers exchange *objects*: one recurring series or single event per
//! remote item, identified by an `href` and versioned by an opaque `etag`.
//! The raw `data` is the provider's own format (iCalendar for CalDAV, JSON
//! for Google and Graph); each provider's [`ObjectCodec`] converts it to and
//! from the provider-neutral model, so the engine never parses provider
//! formats. Writes are conditional on the version the client last saw.
//! See `docs/decisions/0005-google-and-microsoft.md`.

use std::time::Duration;

use std::sync::Arc;

use async_trait::async_trait;
use calendar_core::{ObjectContent, OccurrenceKey};
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

/// One remote object (a single event or a whole recurring series).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteObject {
    pub href: String,
    pub etag: Option<String>,
    /// Raw provider data, read with the provider's [`ObjectCodec`].
    pub data: String,
}

/// An object read into the provider-neutral model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadObject {
    pub content: ObjectContent,
    /// What the model only approximates (shown to the user as such).
    pub warnings: Vec<String>,
}

/// Converts a provider's raw object data to and from the neutral model.
/// Pure and synchronous: the engine runs it on a blocking thread.
pub trait ObjectCodec: Send + Sync {
    fn read(&self, data: &str) -> Result<ReadObject, String>;

    /// The upload payload for `content`. `base` is the last server copy
    /// (`None` for a new object); properties this application does not
    /// model must survive. `renamed` maps overrides' current recurrence ids
    /// to the ones they have in `base` (the series was moved).
    fn write(
        &self,
        base: Option<&str>,
        uid: &str,
        content: &ObjectContent,
        renamed: &[(OccurrenceKey, OccurrenceKey)],
    ) -> String;
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
    /// Where the object now lives; differs from the request for providers
    /// that assign ids on create.
    pub href: String,
    /// New version, if the server returned one.
    pub etag: Option<String>,
    /// The server's copy after the write, if the provider has it.
    pub data: Option<String>,
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
    /// Reads and writes this provider's object data.
    fn codec(&self) -> Arc<dyn ObjectCodec>;

    /// A changed account secret (e.g. rotated OAuth tokens) to store in
    /// place of the one the provider was created with. Taken once.
    fn take_updated_secret(&self) -> Option<String> {
        None
    }

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

    /// Creates or replaces an object with a payload from [`ObjectCodec::write`].
    async fn put_object(
        &self,
        href: &str,
        payload: &str,
        precondition: &Precondition,
    ) -> ProviderResult<WriteResult>;

    /// The current version of one object, or `None` if it does not exist.
    /// Used when the client does not know an object's version (e.g. the
    /// server answered a write without an ETag, RFC 4791 §5.3.4).
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>>;

    /// Deletes an object; `version` makes the delete conditional.
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()>;
}
