# calendar-storage

**Where:** `crates/calendar-storage/`
**Depends on:** `calendar-core`, `sqlx` (async SQLite), `tokio`, `serde_json`.
**Used by:** `calendar-sync` and the desktop app.

## Its job in the architecture

This is the **local-first** part. The SQLite database (`~/.local/share/org.universalcalendar.desktop/calendar.db`) is the **single source of truth for what you see**. The UI never waits for a server: it reads from here, writes here, and the sync engine reconciles with servers later.

Three guarantees live in this crate:

1. **Queries are always bounded:** "events between these two dates", never "everything".
2. **Related writes happen in one transaction.** Saving an event and queuing its upload either both happen or neither does, even if the app crashes.
3. **Heavy CPU work is kept off the async threads.** Recurrence expansion runs on Tokio's blocking pool, at most two at a time.

It knows nothing about CalDAV, Google or HTTP. The sync engine passes in already-parsed content.

## Files

### `lib.rs`: the `Store` and everything the UI does

`Store` is a cheap-to-clone handle to the database:

```rust
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,                                   // up to 4 SQLite connections
    recurrence_checks: Arc<RwLock<HashMap<ProviderKind, RecurrenceCheck>>>,
    reminder_limits:   Arc<RwLock<HashMap<ProviderKind, usize>>>,
    expansions: Arc<tokio::sync::Semaphore>,            // at most 2 expansions at once
}
```

Main methods:

- **Reading:** `query_range` (a visible date range), `search`, `list_calendars`, `settings`.
- **Writing:** `create_event`, `update_event`, `update_series`, `update_following`, `update_occurrence`, `delete_*`, `set_setting`.
- `open` sets up SQLite (WAL mode, foreign keys) and runs the **migrations**.

How `query_range` keeps the UI fast:

```rust
let permit = Arc::clone(&self.expansions).acquire_owned().await?;   // wait for a slot
tokio::task::spawn_blocking(move || {                                 // run on a CPU thread
    let _permit = permit;                                             // released when done
    for row in rows { result.extend(recurrence::expand(&event, &range, &display_tz, skip)?); }
    …
}).await
```

A typical write:

```rust
let mut tx = self.pool.begin().await?;      // start a transaction
Self::writable_calendar(&mut tx, …).await?; // refuse read-only calendars
Self::update_row(&mut tx, &updated).await?;
Self::touch_event(&mut tx, …).await?;       // queue the upload, if synced
tx.commit().await?;                         // all or nothing
```

If any `?` returns early, `tx` is dropped without `commit()`, and SQLx rolls the transaction back automatically. That's ownership doing the cleanup ([concepts §1](concepts.md#1-variables-ownership-and-borrowing)).

**Rust concepts:**
- **`#[derive(Clone)]` on a handle:** everything inside is either a pool (already shared) or an `Arc`, so cloning shares rather than copies ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **`async fn` everywhere:** database calls wait on disk without blocking a thread ([§14](concepts.md#14-asyncawait-and-tokio)).
- **`spawn_blocking` plus a `Semaphore`:** CPU work off the async threads, with a limit.
- **`static MIGRATOR = sqlx::migrate!("./migrations")`:** the SQL files are built into the program at compile time.
- **A type alias for errors:** `pub type Result<T> = std::result::Result<T, StorageError>;` so signatures read `-> Result<Vec<Calendar>>`.
- **Function pointers:** `RecurrenceCheck` is a plain `fn(…) -> …` type. The sync engine registers Microsoft's "can Outlook hold this rule?" function (`calendar_microsoft::pattern::check`), so storage can enforce it without depending on the Microsoft crate.
- **Poisoned locks:** `.unwrap_or_else(PoisonError::into_inner)` keeps working even if another thread panicked while holding the lock.
- **SQL with bound parameters** (`.bind(calendar_id)`), never string concatenation, so there's no SQL injection.

### `remote.rs`: the storage side of syncing

Everything the sync engine needs to remember:

- remote accounts and calendars (server URL, sync token, colour);
- for each event, its server **href** (address) and **etag** (version);
- the **operation queue** (`PendingOp`): uploads and deletes waiting to be sent, with attempt counts for retry and backoff;
- **conflicts** and the server's copy of a conflicting version (`record_conflict_copy`);
- `apply_remote_changes`: writes a batch of server changes in **one transaction**. It skips objects with local changes still queued, so your edit is never overwritten while waiting to upload.

```rust
pub enum Presence {
    Local,    // created here, never sent
    Sent,     // create sent; did it arrive? unknown
    Present,  // exists on the server
}
```

The `Sent` state exists because a network failure can happen *after* the server saved the event but *before* we heard back. The next pass checks before creating again, so retries never make duplicates.

**Rust concepts:**
- **A second `impl Store` block in another file:** Rust lets you split a type's methods across modules.
- **`pub(crate)` helpers** used by `lib.rs` but hidden from other crates ([§8](concepts.md#8-modules-and-visibility)).
- **`&mut Transaction<'_, Sqlite>` parameters:** helpers borrow the caller's transaction, so many steps join one commit. `'_` means "some lifetime, let the compiler work it out".
- **Enums as state machines** (`Presence`, `OpKind`, `Resolution`).

### `codec.rs`: rows ⇄ models

Converts between a database row (`EventRow`, plain strings and numbers) and `calendar-core`'s rich types (`CalendarEvent` with its `EventTiming` enum).

```rust
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct EventRow { pub id: String, pub timing_kind: String, pub start_value: String, … }
```

**Rust concepts:** a **derive from another library** (`sqlx::FromRow` writes the "row to struct" code). It also keeps the database layout separate from the domain model, as `wire.rs` does for the UI.

### `tests.rs`: storage tests

Over 2,000 lines of tests, included only for `cargo test` via `#[cfg(test)] mod tests;` at the bottom of `lib.rs`. Each test opens a fresh **in-memory database** (`Store::open_in_memory()`), so tests are fast and independent. A nested `mod sync` groups the sync-related ones.

**Rust concepts:** `#[tokio::test]` for async tests; `use super::*;` to reach private items of the parent module ([§17](concepts.md#17-tests)).

### `migrations/*.sql`

Not Rust, but important: each file changes the database layout once, in date order. SQLx records which ones have run. Never edit an old migration; add a new one.

### `examples/bench_queries.rs`

A small program that fills a database with 100, 1,000 and 5,000 simulated events and times queries. Run it with:

```sh
cargo run --release -p calendar-storage --example bench_queries
```

**Rust concepts:** `examples/` programs; `sort_by(f64::total_cmp)`, because floating-point numbers aren't `Ord` (NaN can't be ordered), so you choose an ordering explicitly.
