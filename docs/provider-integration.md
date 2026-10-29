# Provider integration

> **Status:**
> - **CalDAV:** implemented (Milestone 3) in `crates/calendar-caldav`.
> - **Google and Microsoft:** Milestone 4.

## Boundaries

- Provider adapters are separate crates (`calendar-caldav`, `calendar-google`, `calendar-microsoft`, later ICS). They depend on `calendar-provider-api` and `calendar-core`, **never** on Tauri or the UI.
- Adapters translate between provider formats and `calendar-core` types. They also keep the provider metadata needed for lossless round trips: ETags, change keys, raw iCalendar properties and provider IDs.
- The UI sees only provider-neutral types. On the frontend, the `EventSource` interface (`apps/desktop/src/services/eventSource.ts`) is the data boundary.

## Provider contract

Defined in `crates/calendar-provider-api`.

```rust
#[async_trait]
pub trait CalendarProvider: Send + Sync {
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>>;
    /// Changes since `cursor` (None = full listing). `known` = (href, etag) the client holds,
    /// so providers without cursors can download only what differs and report deletions.
    async fn fetch_changes(&self, calendar: &RemoteCalendar, cursor: Option<&str>,
                           known: &[(String, Option<String>)]) -> ProviderResult<ChangeSet>;
    async fn put_object(&self, href: &str, ical: &str, precondition: &Precondition)
        -> ProviderResult<WriteResult>;   // Precondition::Create | Version(etag) | Overwrite
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()>;
}
```

Providers exchange **calendar object resources**: an iCalendar object per item, with an `href` and an opaque `etag`.

- **CalDAV and ICS** are natively like this.
- **JSON APIs** (Google, Graph) will map their events to iCalendar objects inside their adapter. The sync engine, the offline queue and conflict handling then stay identical for every provider.
- **Implication for Milestone 4:** those adapters need a careful JSON ⇄ iCalendar mapping, including their recurrence exceptions. Their opaque version tokens (Google `etag`, Graph `changeKey`) take the place of ETags.

`ProviderError` classifies failures so the engine can choose between retry with backoff, conflict, re-authentication and reporting:

- `Unauthorized`
- `Forbidden`
- `NotFound`
- `PreconditionFailed`
- `SyncTokenInvalid`
- `RateLimited { retry_after }`
- `Network`
- `Server`
- `InvalidResponse`

`ProviderError::is_transient()` marks the retryable cases.

## Authentication

| Provider | Mechanism | Notes |
|---|---|---|
| CalDAV (Nextcloud, Fastmail, Radicale, Baïkal, iCloud) | Basic auth over TLS with app passwords | Discovery via `.well-known/caldav` and principal/home-set lookup. No Nextcloud-specific assumptions. |
| Google Calendar | OAuth 2.0 installed-app flow with PKCE, loopback redirect | Users or distributors register their own OAuth client. No client secret is treated as confidential. |
| Microsoft Graph | OAuth 2.0 public client with PKCE (MSAL-style) | Personal and work/school accounts, subject to tenant policy. |
| ICS subscription | Optional HTTP auth | Read-only. Uses ETag / Last-Modified. |

Tokens and passwords live **only** in the OS secret store: the Secret Service API (KWallet / GNOME Keyring) on Linux, via the `keyring` crate or equivalent. Nothing secret goes into SQLite or logs.

## Adding a provider

1. Create the crate `calendar-<name>` implementing `CalendarProvider`.
2. Declare capabilities honestly; the UI greys out unsupported actions.
3. Add a mapping layer to and from `calendar-core` with round-trip tests against recorded fixtures.
4. Add a mock-server integration test. Real credentials must never be required for CI.
5. Document registration requirements, for example the OAuth client setup, in this file.
