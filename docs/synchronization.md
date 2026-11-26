# Synchronization

> **Status:**
> - **CalDAV:** implemented (Milestone 3). Tested against Radicale 3.9, a mock server replaying Nextcloud responses, and a live Nextcloud.
> - **Google and Microsoft:** use the same engine (Milestone 4); see [ADR 0005](decisions/0005-google-and-microsoft.md). Google has been checked with a live account; Microsoft so far only against an API fake.

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
- **Presence is tracked separately from the version.** `remote_objects.presence` is `local` (never sent), `sent` (a create went out; the answer may have been lost) or `present`. An upload of a `local` object first marks it `sent` in its own transaction. So a deletion made while the first upload is in flight is still queued and sent to the server.
- **Local-only objects.** Deleting an object that is still `local` simply forgets it.
- **Unknown versions.** If a write is answered without an ETag (allowed by RFC 4791 §5.3.4), the engine reads the object back to learn its version and the server's copy. If the version is still unknown at the next upload, it asks the server first and never sends a create for an object that may already exist.
- **Edits during an upload.** Each operation has a generation counter. A change made while an upload is running bumps it, so that upload doesn't mark the newer change as synced and it is uploaded next.
- **Preserving uploads.** The object is rebuilt from the last server copy plus the local rows (`calendar-ical::write_object`). Only changed properties are rewritten and SEQUENCE is incremented, so alarms, attendees and vendor properties survive. When a whole series moves, its overrides get new recurrence ids. `events.base_recurrence_id` remembers which component of the server copy each one came from, so that component is edited, not replaced.
- **Crash-safe.** The queue survives restarts and crashes because it is in SQLite (WAL).

## Uploads and conflicts

| Case | Request |
|---|---|
| Create | `PUT` with `If-None-Match: *` |
| Update | `PUT` with `If-Match: <stored ETag>` |
| Delete | `DELETE` with `If-Match` |

| Server answer | Result |
|---|---|
| `412 Precondition Failed` on an update | **merge attempt** (below); if that fails, **conflict**: the operation and its events are marked `conflict` and the server copy isn't touched |
| `412` on a create or delete | **conflict** |
| Network error, 5xx, 429/503 | retried with backoff (30 s doubling to 1 h, or the server's `Retry-After`); the pass stops and the account shows *Offline* |
| 401 | the account shows a credentials error; the queue is kept |
| Other 4xx (e.g. 403 on a read-only calendar) | `sync-failed`; the user can retry or discard |

**Merging.** When an update is refused because the server copy changed, the engine fetches the server's current version. It then compares, per event component (the master and each override) and per field, the local version and the server's version against the last common copy (`calendar_core::merge::three_way`):

- A field changed on only one side takes that side's value.
- Cancelled occurrences combine as sets.
- A field changed differently on both sides, or a component deleted on one side and changed on the other, is a conflict.

Without a conflict, the merge is built on the server's copy (so its unknown properties survive), uploaded against the server's version, and imported locally. Nothing has to be resolved by hand. If the event was edited again meanwhile, nothing local is replaced: the next upload conflicts and merges again. A series moved locally (its occurrence ids changed) is not merged.

When a conflict remains, the server's version is kept (`remote_objects.conflict_ical`). The editor's **Compare versions** then shows both versions side by side, with the changed fields marked.

**Resolving** (editor banner, or **Review…** in Settings → Accounts, a list that also shows deletions that have no event left):

- **Keep mine** (conflicts): uploads the local version unconditionally.
- **Try again** (refused changes): retries the same conditional request.
- **Use server version / Discard my change / Restore the event:**
  - drops the local change;
  - forgets the stored version (the object stays known, with an unknown version);
  - resets the calendar's sync token.

  The next full listing then downloads the server copy, or finds it gone and removes the local rows. A create that never reached the server is simply removed.
- **Further edits** to an object in conflict keep it in conflict until it is resolved.

**During a pull, objects with a queued local change are left untouched.** The version check on upload turns any concurrent server change into a conflict rather than an overwrite.

## Failure recovery

| Failure | Behaviour |
|---|---|
| Network down | Changes stay queued (`pending-sync`); status *Offline*; retry with backoff or on *Sync now* (which skips the wait) |
| Password rejected / revoked | Account shows the error; nothing is lost; other accounts keep syncing |
| Calendar deleted remotely | Removed locally unless it has queued changes (then kept) |
| Lost access to one calendar | Skipped; the rest of the account syncs |
| Malformed remote data | The last good version of that object is kept (rows, ETag, base copy) and the failure is recorded in `remote_objects.import_error` and counted in Settings → Accounts. The rest is applied. Parsing runs off the async workers, with checked arithmetic and a panic guard per object. |
| Data read only approximately | Recorded in `remote_objects.import_warnings` and counted in Settings → Accounts |
| An account keeps failing | Account-level backoff (30 s doubling to 1 h); a local edit or *Sync now* retries at once |
| A pass hangs or panics | Each account's pass runs as its own task with a 10-minute limit; a panic ends that pass, not the scheduler. Up to 3 accounts sync in parallel. |
| Crash during sync | Pull transactions are atomic; queued uploads are replayed |
| Password missing from keyring | Account shows *Could not read the account password*; re-add the account |

## Two programs, one database

The desktop app and `ucal` (the terminal tool) open the same SQLite database. Reading and writing at the same time is safe: SQLite runs in WAL mode with a busy timeout, and every related write is one transaction.

Syncing needs more. The engine's per-account lock lives in memory, so it only sees its own process. Two programs running a pass for the same account at once could both upload the same queued change. So each pass also takes an **OS file lock** for its account (`sync-<account id>.lock` next to the database, `SyncEngine::with_lock_dir`).

- A second program's pass **waits** for the first, then finds the queue already sent and has nothing to upload. The wait counts towards the 10-minute pass limit.
- Different accounts still sync in parallel.
- The OS releases the lock when the program exits, even after a crash, so a stale lock file is harmless.
- Programs that share a database must use the same lock directory. Tests using their own `UC_DATA_DIR` have their own.

Changes `ucal` saves are picked up by the desktop app's next pass (within 5 minutes), by `ucal sync`, or by `ucal`'s full-screen view, which runs the same scheduler in the background while it is open. The desktop window shows them when it next loads that range.

## Reading again after an update

Each calendar remembers the version of the provider readers its events were read with (`calendars.reader_version`). When an update teaches the readers something new, `calendar-sync::READER_VERSION` goes up. The next pass then lists such a calendar in full and treats every object as changed, so existing events gain the new data. Version 1 added reminders. Deletions are detected as usual, and objects with queued local changes are not overwritten.

## Reminder notifications

`calendar-sync::reminders` runs beside the sync scheduler, without polling. It looks for reminders due in the window since its last check, shows them through the desktop shell, and sleeps until the next one (at most an hour). Local edits and sync passes that changed data wake it early. Because the window is half-open, no reminder is shown twice, and none is skipped when the loop runs late. After the computer was asleep, missed reminders are shown only for events that haven't ended.

## Moving events between calendars

Choosing another calendar for an event (or a whole series) moves it. This works between any two writable calendars: local ones, calendars of one account, or of different accounts and providers. Either way, a series moves as a whole, with its modified and cancelled occurrences. "Only this one" and "this and following" can't change the calendar.

**Checked first:** a read-only calendar can't receive events, and the new calendar's limits apply (Outlook: one reminder, fewer repeat rules). `Store::update_and_move` saves the other edits of the same save only once these checks pass.

### Within a Google or CalDAV account: the server moves it

Between two calendars of one Google or CalDAV account, the server moves the event: Google `events.move`, WebDAV `MOVE` (with `Overwrite: F` and `If-Match`). It stays **the same event**: same UID and id, and everything the app doesn't model stays with it, such as attendees, conferencing and invitation links.

- **Locally**, the rows and their remote object change calendar at once and keep their ids. One `move` operation is queued in `pending_operations`; its `move_from` names the calendar the server still has the event in.
- **Until the move is sent**, neither calendar's sync changes the event. The old calendar's listing still has it, but it isn't imported there again; the new calendar's listing doesn't have it yet, and it isn't deleted.
- **Moved again before syncing:** the queued move is updated to go from where the server has the event, or dropped if the event went back there. An edit made with the move, or after it, is uploaded right after the move, in the same pass.
- **Version check:** the move is conditional on the version we knew. If someone changed the event on the server meanwhile, the move becomes a conflict ("You moved this event to another calendar, but it was changed on the server"):
  - **Keep mine** moves the server's copy anyway, and then shows the changes made there;
  - **Use server version** puts the event back in its old calendar, as the server has it.
- **Changes during the move** are kept. If the event was edited, moved on or deleted here while the request ran, that change is sent next, to the event's new place on the server.
- **When the server can't move it**, the engine copies it instead (below), in the same pass. This happens for Google events organised by someone else (an invitation), and for CalDAV servers that refuse a `MOVE` between those calendars (403, 405, 501 or 502).
- Outlook (Microsoft Graph) has no move for events: its moves are copies.

### Otherwise: a copy and a deletion

Between accounts, to or from a local calendar, and with Outlook, `Store::move_event` makes the move as a deletion plus a creation, in one transaction:

- The rows are copied into the new calendar under a **new id**. The originals are deleted the usual way.
- **The copy**, in a synchronized calendar, is queued for upload as a new object, with a new UID.
- **The old server copy** is queued for deletion with the version we knew. A change someone made there meanwhile becomes a conflict, shown under *Review…*, instead of being deleted. Keeping the server's version brings it back beside the moved copy, and the new id keeps the two apart. An object that was never uploaded is simply forgotten.
- **The deletion waits for the copy.** It names the copy's queued upload (`after_calendar`, `after_href`, migration 9) and isn't sent while that upload is queued, retrying or refused. Only once the new server has the copy is the original deleted, in the same pass within an account and in the next one (about a second later) for another account. If the new server refuses the copy, the original stays on its server. *Discard my change* on the refused copy, or removing the copy's account, drops the waiting deletion, and the original comes back with the next sync. A copy moved on again before it was uploaded passes the wait on to the next copy.
- The local rows are the source of truth throughout: if the deletion or the upload fails, it is retried, and nothing is lost.
- **Not carried over:** what the app doesn't model (attendees, conferencing, links to invitations). The server sees a new event.

## Splitting a series ("this and following")

A split is two ordinary changes: the original series is uploaded with its shortened rule, and the new series is created like any new event. Both go through the queue, version checks and conflicts as usual.

- **Google and Microsoft:** exceptions on the server that the shortened rule no longer generates are removed explicitly (Google `DELETE`, Graph `DELETE` after the guarded master change), so none are left orphaned. The new series gets its own exceptions.
- **Not carried over:** details this app doesn't edit (attendees, reminders, conferencing) stay with the original series. The new series starts without them, and without links to meeting invitations.

## Not yet supported

- Creating or deleting calendars on the server (MKCALENDAR).
- HTTP Digest authentication.
- Generating VTIMEZONE components for new events (IANA TZIDs are sent by reference, which Nextcloud and Radicale accept).
- Scheduling and invitations (iTIP).
- Editing attendees or alarms (they are preserved, not editable).
