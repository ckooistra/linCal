# Provider integration

> **Status: design outline. Nothing here is implemented yet.**
> Milestone 1 uses simulated data only. CalDAV arrives in Milestone 3, and
> Google and Microsoft in Milestone 4.

## Boundaries

- Provider adapters are separate crates (`calendar-caldav`, `calendar-google`, `calendar-microsoft`, later ICS). They depend on `calendar-provider-api` and `calendar-core`, **never** on Tauri or the UI.
- Adapters translate between provider formats and `calendar-core` types. They also keep the provider metadata needed for lossless round trips: ETags, change keys, raw iCalendar properties and provider IDs.
- The UI sees only provider-neutral types. On the frontend, the `EventSource` interface (`apps/desktop/src/services/eventSource.ts`) is the data boundary.

## Provider contract (initial sketch)

```rust
#[async_trait::async_trait]
pub trait CalendarProvider: Send + Sync {
    fn capabilities(&self) -> &ProviderCapabilities;
    async fn list_calendars(&self) -> Result<Vec<RemoteCalendar>, ProviderError>;
    /// Incremental changes since `cursor` (sync token, delta link, ctag/etag set).
    /// Paginated; `None` cursor means initial sync of `window`.
    async fn fetch_changes(&self, calendar: &RemoteCalendarRef, cursor: Option<&SyncCursor>,
                           window: Option<DateWindow>) -> Result<ChangePage, ProviderError>;
    /// Conditional write: fails with `ProviderError::Conflict` when `expected_version` is stale.
    async fn create_event(&self, calendar: &RemoteCalendarRef, event: &EventPayload)
        -> Result<RemoteVersion, ProviderError>;
    async fn update_event(&self, event: &RemoteEventRef, payload: &EventPayload,
                          expected_version: &RemoteVersion) -> Result<RemoteVersion, ProviderError>;
    async fn delete_event(&self, event: &RemoteEventRef, expected_version: &RemoteVersion)
        -> Result<(), ProviderError>;
}
```

`ProviderCapabilities` describes, per provider and per calendar:

- read-only status;
- recurrence editing support ("this and following");
- attendees and reminders support;
- maximum page size;
- available sync mechanisms: CalDAV sync-token, Google `syncToken`, Graph delta.

`ProviderError` separates the following cases, so the sync engine can choose between retry, backoff, re-authentication and surfacing the error to the user:

- auth expired/revoked
- rate-limited (with retry-after)
- conflict
- not found / calendar deleted
- transient network
- malformed data

## Authentication

| Provider | Mechanism | Notes |
|---|---|---|
| CalDAV (Nextcloud, Fastmail, Radicale, Baïkal, iCloud) | Basic auth over TLS with app passwords | Discovery via `.well-known/caldav` and principal/home-set lookup. No Nextcloud-specific assumptions. |
| Google Calendar | OAuth 2.0 installed-app flow with PKCE, loopback redirect | Users or distributors register their own OAuth client. No client secret is treated as confidential. |
| Microsoft Graph | OAuth 2.0 public client with PKCE (MSAL-style) | Personal and work/school accounts, subject to tenant policy. |
| ICS subscription | Optional HTTP auth | Read-only. Uses ETag / Last-Modified. |

Tokens and passwords live **only** in the OS secret store: the Secret Service API (KWallet / GNOME Keyring) on Linux, via the `keyring` crate or equivalent. Nothing secret goes into SQLite or logs.

## Adding a provider (future checklist)

1. Create the crate `calendar-<name>` implementing `CalendarProvider`.
2. Declare capabilities honestly; the UI greys out unsupported actions.
3. Add a mapping layer to and from `calendar-core` with round-trip tests against recorded fixtures.
4. Add a mock-server integration test. Real credentials must never be required for CI.
5. Document registration requirements, for example the OAuth client setup, in this file.
