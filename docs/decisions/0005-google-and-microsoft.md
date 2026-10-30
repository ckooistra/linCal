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

The Google adapter therefore aggregates. (Graph is handled differently; see the addendum.)

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

## Addendum: Microsoft Graph objects are instances (implemented)

Graph's `calendarView/delta`, which the specification asks for, reports **instances** within a date window: single events, occurrences and exceptions, each with its own id. It does not report series masters with their rules. Reading masters plus `instances` per series would need a request per series on every change, and Graph v1.0 offers no delta for masters.

The Microsoft adapter therefore mirrors instances:

- **One object per instance.** Each instance is read as a single event. Editing it `PATCH`es that instance, which Graph turns into an exception. Deleting it cancels that occurrence.
- **Series are not edited as a whole.** Storage refuses to create repeating events or edit series in Microsoft calendars (`check_series_supported`). Occurrences are flagged *shown approximately*, so the limitation is visible.
- **Window.** The window is month-aligned: 12 months back and 24 months ahead. It moves forward with a full listing once less than 12 months of future is left.

Whole-series editing for Microsoft (master + recurrence pattern ⇄ RRULE) is future work.
