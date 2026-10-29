# Synchronization

> **Status: design outline. Remote synchronization is not implemented yet**
> (Milestones 3–4). Milestone 2 provides the local half: every change is
> committed to SQLite in a transaction before it is shown as `saved`, and the
> UI's optimistic updates roll back if that write fails.

## Principles

- **Local first.** Every user change is committed to SQLite before any network activity. The UI reads from SQLite (through the Rust core), never from providers directly.
- **Never silently lose a change.** Failed or conflicting changes stay visible with their status until they are resolved.
- **Isolate failures.** Each account syncs independently; one broken provider doesn't block the others.
- **No busy polling.** Sync is driven by app start, user actions, a modest timer with backoff, and network-change notifications.

## Event lifecycle (`SyncStatus`)

`pending-local` → `saved` → `pending-sync` → `synced`, or `sync-failed` / `conflict`.

M1 implements `pending-local` → `saved` against the mock, and rolls back on failure.

## Initial sync

1. Discover calendars and store them with their capabilities.
2. Fetch a bounded window of events (for example −6 to +12 months), paginated, through a bounded worker pool.
3. Store each provider's version metadata (ETag, change key) and its sync cursor (CalDAV sync-token, Google `syncToken`, Graph delta link).

## Incremental sync

Fetch changes since the stored cursor and apply them in one transaction per page.

If a cursor is invalidated (HTTP 410 for Google or Graph, invalid sync-token for CalDAV), fall back to a windowed re-sync and reconcile by provider ID.

## Offline changes and the operation queue

- Every local mutation writes the event **and** a `pending_operations` row in one SQLite transaction. The row records the operation, target, payload and expected remote version.
- A background worker drains the queue per account with exponential backoff and jitter. It honours `Retry-After` and rate limits.
- Operations carry a client-generated idempotency key, the event's stable ID. An ambiguous outcome such as a timeout after send is resolved by re-reading the remote state before retrying, to avoid duplicate creates.
- The queue survives restarts and crashes because it lives in SQLite (WAL mode).

## Conflicts

Writes are conditional on the stored version (`If-Match` ETag for CalDAV and Graph, `etag` for Google). On a precondition failure:

1. fetch the remote version;
2. if the fields changed locally and remotely don't overlap, merge automatically;
3. otherwise mark the event `conflict`, keep **both** versions, and let the user choose.

Remote deletion of a locally edited event is always a conflict, never a silent drop.

## Failure recovery

| Failure | Behaviour |
|---|---|
| Network down | Queue keeps the changes; status shows offline; retry on reconnect. |
| Auth expired or revoked | Account marked as needing re-authentication; other accounts keep syncing. |
| Calendar deleted remotely | Local copy marked orphaned; pending changes are offered for export or move, never discarded. |
| Malformed remote data | Item skipped and logged (redacted); the rest of the page is applied. |
| SQLite busy | Bounded retry on a blocking thread pool; the UI is never blocked. |
