# Provider integration

> **Status:**
> - **CalDAV:** implemented (Milestone 3) in `crates/calendar-caldav`.
> - **Google Calendar and Microsoft Graph:** implemented (Milestone 4) in
>   `crates/calendar-google` and `crates/calendar-microsoft`, with OAuth in
>   `crates/calendar-oauth`. Tested against stateful API fakes; a live check
>   needs an OAuth client registration (below).
> - **ICS subscriptions:** implemented (Milestone 5) in `crates/calendar-ics`,
>   read-only. Checked against Google's and Apple's public holiday feeds.

## Boundaries

- Provider adapters are separate crates (`calendar-caldav`, `calendar-google`, `calendar-microsoft`, `calendar-ics`). They depend on `calendar-provider-api` and `calendar-core`, **never** on Tauri or the UI.
- Adapters translate between provider formats and `calendar-core` types. They also keep the provider metadata needed for lossless round trips: ETags, change keys, raw iCalendar properties and provider IDs.
- The UI sees only provider-neutral types. On the frontend, the `EventSource` interface (`apps/desktop/src/services/eventSource.ts`) is the data boundary.

## Provider contract

Defined in `crates/calendar-provider-api`.

```rust
#[async_trait]
pub trait CalendarProvider: Send + Sync {
    /// Reads and writes this provider's raw object data (iCalendar or JSON).
    fn codec(&self) -> Arc<dyn ObjectCodec>;
    /// Rotated OAuth tokens to store, if any (taken once).
    fn take_updated_secret(&self) -> Option<String> { None }
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>>;
    /// Changes since `cursor` (None = full listing). `known` = (href, etag) the client holds,
    /// so providers without cursors can download only what differs and report deletions.
    async fn fetch_changes(&self, calendar: &RemoteCalendar, cursor: Option<&str>,
                           known: &[(String, Option<String>)]) -> ProviderResult<ChangeSet>;
    /// Payload from `ObjectCodec::write`. The result may carry a new href
    /// (server-assigned ids) and the server's copy.
    async fn put_object(&self, href: &str, payload: &str, precondition: &Precondition)
        -> ProviderResult<WriteResult>;   // Precondition::Create | Version(etag) | Overwrite
    /// One object, or None if it does not exist (used when a version is unknown).
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>>;
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()>;
}
```

Providers exchange **objects**: one event or one recurring series per remote item, with an `href`, an opaque `etag` and raw `data` in the provider's own format. Each provider's `ObjectCodec` turns that data into `calendar-core` content and back, so the sync engine, the offline queue, conflict handling and the review dialog are the same for every provider. The decisions behind this are in [ADR 0005](decisions/0005-google-and-microsoft.md).

| | CalDAV | Google Calendar | Microsoft Graph | ICS subscription |
|---|---|---|---|---|
| Object | one iCalendar resource | a series: master + exceptions (`events.list?iCalUID=`) | a single event, or a series: master read with `$expand=exceptionOccurrences` and `cancelledOccurrences` | the feed's VEVENTs with one UID, wrapped in a VCALENDAR with the VTIMEZONEs they use; href `ics:feed/<uid>` (the feed URL may be secret and is never used as an href) |
| Version | ETag | hash of the members' ETags | `@odata.etag`; for a series, a hash over master, exceptions and cancellations | hash of the object's text |
| Cursor | sync-token, or ETag comparison | `syncToken` (410 → full sync) | window + `calendarView/delta` link (410 or window moved → full listing) | the feed's ETag / Last-Modified plus the fetch time: a conditional GET at most every 15 minutes; a changed feed is compared object by object with what the client holds |
| Writes | edit the stored iCalendar in place, `PUT` with `If-Match` | `PATCH` only changed fields per member, with `If-Match` | `PATCH` only changed fields, with `If-Match`; new exceptions and cancellations look the occurrence up via `instances` | refused (read-only calendar) |
| Create identity | client href | client-chosen id (retry → 409) | server id; `transactionId` makes retries safe; href renamed | — |
| Recurrence | full (RRULE/RDATE/EXDATE, overrides) | full (RFC 5545 lines + exceptions) | Outlook patterns ⇄ RRULE (daily, weekly on days, monthly/yearly by day or by first–fourth/last weekday; count or end date). Other rules, extra dates (RDATE) and restoring a cancelled occurrence are refused before they are stored | full (read with the CalDAV codec) |
| Reminders (minutes before the start) | VALARM with ACTION DISPLAY/AUDIO and a TRIGGER at or before the start; only those are rewritten when reminders change | `reminders.overrides` (popup), or the calendar's `defaultReminders` when `useDefault` (stored with each object); a change writes `useDefault: false` with popup overrides | `isReminderOn` + `reminderMinutesBeforeStart`: one reminder (storage refuses more) | as CalDAV, read-only |
| Unmodelled data (attendees, alarms, conferencing…) | preserved in the stored resource | untouched on the server | untouched on the server | kept in the stored copy |

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
| CalDAV (Nextcloud, Fastmail, Radicale, Baïkal, iCloud) | Basic auth over TLS with app passwords | Discovery via `.well-known/caldav` and principal/home-set lookup. No Nextcloud-specific assumptions. HTTPS is required except for `localhost`. A calendar home on another host is accepted only within the same domain and over HTTPS. Addresses with embedded credentials are refused. |
| Google Calendar | OAuth 2.0 installed-app flow with PKCE, loopback redirect `http://127.0.0.1:<port>` | Users or distributors register their own OAuth client. No client secret is built in; Google's desktop-client "secret" is non-confidential and, if entered, is kept in the keyring. |
| Microsoft Graph | OAuth 2.0 public client with PKCE, loopback redirect `http://localhost:<port>` | Personal and work/school accounts (`/common`), subject to tenant policy. Scopes `offline_access User.Read Calendars.ReadWrite`. |
| ICS subscription | None (the address itself may be secret; it is stored in the keyring) | Read-only. Conditional GET with ETag / Last-Modified, gzip, at most 20 MB. `webcal://` means HTTPS; plain HTTP only for localhost, and redirects may not downgrade. HTTP authentication is not supported yet. |

Tokens and passwords live **only** in the OS secret store: the Secret Service API (KWallet / GNOME Keyring) on Linux, via the `keyring` crate or equivalent. Nothing secret goes into SQLite or logs.

An expired or revoked OAuth sign-in shows **Sign in again…** next to the account. Queued changes are kept and sync resumes after signing in.

## Registering OAuth clients

The app never ships a confidential secret. You (or whoever distributes a build) register a client once. Then either enter its ID in the **Add … account** dialog, or build it in with `UC_GOOGLE_CLIENT_ID=… UC_MICROSOFT_CLIENT_ID=… npm run tauri build`.

**Google**

1. In the Google Cloud console, create a project and enable the **Google Calendar API**.
2. Configure the OAuth consent screen. While the app is in testing, add your own address as a test user.
3. Under *Credentials*, create an **OAuth client ID** of type **Desktop app**.
4. Enter the client ID, and the client secret Google shows, in **Add Google account…**. Google documents desktop secrets as non-confidential; it is stored in the keyring, not the database.

**Microsoft**

1. In the Microsoft Entra admin center, go to *App registrations* → *New registration*.
2. Under supported account types, choose *Accounts in any organizational directory and personal Microsoft accounts*.
3. Under *Authentication*, add the platform **Mobile and desktop applications** with the redirect URI `http://localhost`, and enable *Allow public client flows*.
4. Under *API permissions*, add the delegated Microsoft Graph permissions `Calendars.ReadWrite`, `User.Read` and `offline_access`.
5. Enter the *Application (client) ID* in **Add Microsoft account…**. No secret is used.

## Adding a provider

1. Create the crate `calendar-<name>` implementing `CalendarProvider`.
2. Declare capabilities honestly; the UI greys out unsupported actions.
3. Add a mapping layer to and from `calendar-core` with round-trip tests against recorded fixtures.
4. Add a mock-server integration test. Real credentials must never be required for CI.
5. Document registration requirements, for example the OAuth client setup, in this file.
