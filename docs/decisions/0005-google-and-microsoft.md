# ADR 0005: Google Calendar and Microsoft Graph (Milestone 4)

Status: accepted (October 2026)

## Context

Milestone 3 built the sync engine around CalDAV: one iCalendar object per `href`, an opaque ETag, conditional `PUT`/`DELETE` and one cursor per calendar. The October audit (F21) asked for these contract questions to be settled before adding JSON providers:

- native-data preservation;
- provider-assigned identity;
- scoped cursors;
- authentication lifecycle.

## Decisions

### 1. Providers own their data format

`RemoteObject.data` is the provider's raw representation:

- iCalendar text for CalDAV;
- JSON for Google and Graph.

Each provider supplies an `ObjectCodec`:

- `read(data) → ObjectContent + warnings` converts the raw data to the provider-neutral model.
- `write(base, uid, content, renamed) → payload` turns the model back into something the provider can upload.

The engine never parses or builds provider formats itself. Storage keeps the raw base copy per object as before.

**Native-data preservation** follows from this. A JSON provider's payload is a list of *patches*: only the fields this app models and actually changed. Everything else (attendees, conferencing, reminders, extended properties) stays on the server untouched. No reversible envelope is needed.

### 2. Recurring series are one object

The engine's unit is a series: a master plus its overrides. CalDAV stores a series as one resource. Google and Graph store the master and each exception as separate events.

Both adapters therefore aggregate (for Graph, see the addendum).

- **Object id and version.** A series' `href` is the master's id. Its version is a hash of the member versions.
- **Grouping.** Changed members found by incremental sync are grouped by series, and the whole series is then read again:
  - Google: `events.list?iCalUID=`.
- **Writes** go per member, each conditional on that member's own version.

A failure halfway through a multi-request write leaves the series partly updated. The retry is then refused by version checks and becomes a conflict, which the user resolves as usual.

Google recurrence is expressed as RFC 5545 lines (`RRULE`, `EXDATE`, `RDATE`). The Google codec builds a VEVENT from the JSON and reuses `calendar-ical`, so recurrence and time-zone handling exist only once.

### 3. Identity on create

- **Google:** the client may choose the event `id` (base32hex), so creates keep their local `href` and a repeated insert answers 409 instead of duplicating.
- **Graph:** the server assigns ids. Creates send `transactionId` (the local UID) so a retried create is not duplicated. The provider returns the new id in `WriteResult.href`, and storage renames the object (`rename_object`).

### 4. Cursors are opaque and provider-scoped

The engine stores one opaque cursor string per calendar.

- **Google:** the cursor is the `syncToken`. HTTP 410 means start a full sync.
- **Graph:** `calendarView/delta` is bound to a date window. The cursor encodes the window and the delta link. When the window must move forward, the provider does a full listing and reports a new cursor. Deletions outside the new window are reported as such.

### 5. OAuth for installed apps

`calendar-oauth` implements the authorization-code flow with PKCE (S256) and a loopback redirect (`http://127.0.0.1:<random port>`, RFC 8252):

- the system browser is opened via a callback supplied by the Tauri layer;
- the flow can be cancelled.

**Client registration:**

- Client ids come from the user or a distributor: entered in the Add account dialog, or built in with `UC_GOOGLE_CLIENT_ID` / `UC_MICROSOFT_CLIENT_ID`.
- No client secret is embedded in the application.
- Google's desktop clients are issued a secret that Google itself documents as non-confidential. If the user enters one, it is stored in the keyring together with the tokens.

**Token storage and refresh:**

- Tokens (access, refresh, expiry) live only in the OS keyring, as one JSON value per account.
- Providers refresh access tokens themselves. Rotated refresh tokens are handed back to the engine (`take_updated_secret`), which saves them.
- A refused refresh is `Unauthorized`: the account shows *Sign in again*, and nothing queued is lost.

### 6. Capabilities

Per-calendar access (owner / writer / reader / free-busy) maps to `read_only`, which storage and UI already enforce.

## Consequences

- CalDAV behaviour is unchanged. Its codec wraps the existing `calendar-ical` functions.
- Every engine path (queue, conflicts, presence tracking, review dialog) applies to all providers.
- Live testing against Google and Microsoft needs OAuth client registrations, which the user or distributor provides. Automated tests use recorded, API-shaped responses from a mock server.

## Addendum: Microsoft Graph series (implemented)

Graph's `calendarView/delta`, which the specification asks for, reports **instances** within a date window: single events, occurrences and exceptions, each with its own id. It does not report series masters with their patterns. A first version therefore mirrored instances one by one and refused series edits. That is replaced by whole series, as for Google:

- **Objects.** An object is a single event, or a series: the master read with `$expand=exceptionOccurrences` and `cancelledOccurrences` selected, so pattern, exceptions and cancellations arrive in one request. The series version is a hash over the master's ETag, the exceptions' change keys and the cancellations.
- **Sync.** Delta entries for occurrences and exceptions name their `seriesMasterId`; each series touched is read again once. Removed entries carry only an id; when one is not a known object, the known series are read again to find the cancelled occurrence or deleted series.
- **Recurrence.** Outlook patterns are translated to RRULE and back (`calendar-microsoft::pattern`), normalised so the editor's presets round-trip. Rules Outlook cannot express, extra dates (RDATE) and restored occurrences are refused by storage before they are queued, through a per-provider check the sync engine registers (`Store::set_recurrence_check`). Writes send local times in the event's zone (Windows names where known) so series follow its daylight-saving rules.
- **Writes.** The master and existing exceptions are `PATCH`ed with `If-Match`. Graph cannot address an occurrence by its original start, so new exceptions and cancellations name the occurrence key and the provider finds the instance via `instances`. Graph may adjust a series beyond what was sent (e.g. drop exceptions when the pattern changes), so a written series is reported with an unconfirmed version and the next sync reads and applies the server's result.
- **Migration.** Microsoft calendars lose their sync state once (migration `20261031000005`), so occurrence objects stored by the first version are replaced by series on the next sync.
- **Window.** The window is month-aligned: 12 months back and 24 months ahead. It moves forward with a full listing once less than 12 months of future is left. Series objects hold all their occurrences, including those outside the window.
