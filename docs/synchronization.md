# Synchronization

> **Status:**
> - **CalDAV:** implemented (Milestone 3). Tested against Radicale 3.9 and a mock server replaying Nextcloud responses; not yet against a live Nextcloud.
> - **Google and Microsoft:** follow in Milestone 4 on the same engine.

## Principles

- **Local first.** Every user change is committed to SQLite before any network activity. The UI reads from SQLite (through the Rust core), never from providers directly.
- **Never silently lose a change.** Failed or conflicting changes stay visible with their status until they are resolved.
- **Isolate failures.** Each account syncs independently; one broken provider doesn't block the others.
- **No busy polling.** Sync runs on start-up, about 1.5 s after local edits (debounced), every 5 minutes, when a queued retry becomes due, and on **Sync now**.

## Event status (`SyncStatus`)

| Status | Meaning |
|---|---|
| `pending-local` | UI only, while the local write is in flight |
| `saved` | Local calendars: stored in SQLite |
| `pending-sync` | Remote calendars: stored locally, upload queued |
| `synced` | Matches the server |
| `sync-failed` | The server refused the change (e.g. permission); the user decides |
| `conflict` | Changed both here and on the server |

## Initial sync

Adding an account works through these steps:

1. **Discovery:** `current-user-principal` → `calendar-home-set`, trying `/.well-known/caldav`. Only same-origin redirects are followed, so credentials never go to another host.
2. **Calendar listing** with name, colour, read-only status (from `current-user-privilege-set`) and sync support. Collections that can't hold events are skipped.
3. **Storage:** the account config goes to SQLite and the password to the system keyring, nothing until both succeed.

The first pull lists all objects via an empty sync token (or a PROPFIND of ETags), fetches them in batches of 50 with `calendar-multiget`, and imports them.

Objects that can't be parsed are recorded with their ETag, so they aren't re-downloaded on every pass, and logged.

All event history is synced (the database is the local source). Rendering stays range-limited.

## Incremental sync

- **With a sync token (RFC 6578):** `sync-collection` returns changed hrefs with ETags and deleted members. Only objects whose ETag differs from the stored one are downloaded. Truncated (507) responses are continued.
- **Expired token:** a full listing is compared with the stored ETags, and anything no longer listed is deleted locally.
- **Applying changes:** each calendar's changes are applied in one transaction, with a savepoint per object so one malformed object can't abort the batch.

## Offline changes: the operation queue

- **Same transaction.** Every local mutation on a synchronized calendar writes the event rows and a `pending_operations` row in the same SQLite transaction.
- **One operation per remote object:**
  - `put` uploads the object's current rows;
  - `delete` removes it.
- **Coalescing.** Repeated edits merge into one upload.
- **Local-only objects.** Deleting an object that never reached the server simply forgets it.
- **Edits during an upload.** Each operation has a generation counter. A change made while an upload is running bumps it, so that upload doesn't mark the newer change as synced and it is uploaded next.
- **Lossless uploads.** The object is rebuilt from the last server copy plus the local rows (`calendar-ical::write_events`). Only changed properties are rewritten and SEQUENCE is incremented, so alarms, attendees and vendor properties survive.
- **Crash-safe.** The queue survives restarts and crashes because it is in SQLite (WAL).

## Uploads and conflicts

| Case | Request |
|---|---|
| Create | `PUT` with `If-None-Match: *` |
| Update | `PUT` with `If-Match: <stored ETag>` |
| Delete | `DELETE` with `If-Match` |

| Server answer | Result |
|---|---|
| `412 Precondition Failed` | **conflict**: the operation and its events are marked `conflict` and the server copy isn't touched |
| Network error, 5xx, 429/503 | retried with backoff (30 s doubling to 1 h, or the server's `Retry-After`); the pass stops and the account shows *Offline* |
| 401 | the account shows a credentials error; the queue is kept |
| Other 4xx (e.g. 403 on a read-only calendar) | `sync-failed`; the user can retry or discard |

**Resolving a conflict** (editor banner):

- **Keep mine:** uploads the local version unconditionally.
- **Use server version:** drops the local change, forgets the stored ETag and resets the calendar's sync token. Server changes skipped while the edit was queued are then fetched by a full ETag comparison.

**During a pull, objects with a queued local change are left untouched.** The version check on upload turns any concurrent server change into a conflict rather than an overwrite.

## Failure recovery

| Failure | Behaviour |
|---|---|
| Network down | Changes stay queued (`pending-sync`); status *Offline*; retry with backoff or on *Sync now* (which skips the wait) |
| Password rejected / revoked | Account shows the error; nothing is lost; other accounts keep syncing |
| Calendar deleted remotely | Removed locally unless it has queued changes (then kept) |
| Lost access to one calendar | Skipped; the rest of the account syncs |
| Malformed remote data | Object skipped and logged; the rest is applied |
| Crash during sync | Pull transactions are atomic; queued uploads are replayed |
| Password missing from keyring | Account shows *Could not read the account password*; re-add the account |

## Not yet supported

- Moving events between calendars once a synchronized calendar is involved (needs a server-side move, or delete + create).
- Creating or deleting calendars on the server (MKCALENDAR).
- HTTP Digest authentication.
- Generating VTIMEZONE components for new events (IANA TZIDs are sent by reference, which Nextcloud and Radicale accept).
- Scheduling and invitations (iTIP).
- Editing attendees or alarms (they are preserved, not editable).
