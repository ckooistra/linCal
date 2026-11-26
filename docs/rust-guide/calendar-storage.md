# calendar-storage

**Where:** `crates/calendar-storage/`
**Depends on:** `calendar-core`, `sqlx` (async SQLite), `tokio`, `serde_json`.
**Used by:** `calendar-sync`, the desktop app and `ucal`.

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
- the **operation queue** (`PendingOp`): uploads, deletes and moves waiting to be sent, with attempt counts for retry and backoff. A moved event's operation carries a `PendingMove` (where the server has it, where it goes), and `complete_move` records the result;
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

### `moving.rs`: moving events to another calendar

`Store::move_event(id, new_id, calendar_id)` moves an event, or a whole series with its exceptions, in **one transaction**, one of two ways.

**Within an account whose server can move events** (`moves_on_server` decides; the sync engine registers CalDAV and Google with `set_moves_on_server`), `relocate`:
1. changes the calendar of the rows and of their remote object, keeping the ids;
2. queues a `move` operation whose `move_from` says where the server still has it (or updates an operation already queued, or drops it if the event went back there).

**Otherwise**, `copy_to` makes it a deletion plus a creation:
1. the rows are copied into the new calendar under `new_id` (exception ids are rebuilt from it with `occurrence_id`);
2. the originals are deleted with the same helpers as `delete_event`, which queue the server deletion;
3. in a synchronized calendar, `attach_new_object` queues the copy's upload;
4. `delete_after_copy` makes the server deletion wait for that upload (`after_calendar`, `after_href`). `due_operations` leaves out operations whose upload they wait for is still queued (the `not_waiting!` SQL fragment). `complete_put` lets them go (`copy_uploaded`). Giving the copy up (`resolve_operation` with `UseServer`, or `remove_account`) drops them and lists the original's calendar afresh (`keep_originals`), so a refused copy never costs the original.

Here a new id is essential: if the old server copy came back (a conflict resolved by keeping the server's version), it would otherwise collide with the moved event.

If the server refuses a move after all, the engine calls `copy_instead_of_move`, which puts the rows back where the server has them and runs `copy_to`.

`Store::update_and_move` is what the editors call. It checks the new calendar first, saves the other edits, then moves.

**Rust concepts:**
- **Struct update syntax with a closure:** `let copy = |event: &CalendarEvent, id, series| CalendarEvent { id, calendar_id: …, series, ..event.clone() };` builds each copy from the original, changing only what a move changes.
- **`std::iter::once(&master).chain(&overrides)`** checks the master and every exception with one loop ([concepts §11](concepts.md#11-iterators)).
- **`mut` parameters:** `mut draft: EventDraft` lets the function change its own copy of the argument (`draft.calendar_id = from`) without the caller seeing it ([§1](concepts.md#1-variables-ownership-and-borrowing)).
- **Matching on a tuple with a guard:** `Some((kind, origin)) if origin.as_deref().unwrap_or(from) == to && kind == "move"` picks out "moved back before it was sent" before the general `Some((_, origin))` arm ([§4](concepts.md#4-match-and-pattern-matching)).
- **`HashSet<ProviderKind>` behind an `RwLock`:** which providers move on the server is registered once and read on every move, so many readers can look at the same time.

### `import.rs`: importing events from a file

`Store::import_objects(calendar_id, objects)` stores events read from an `.ics` file, in **one transaction**. Each event becomes a new local event (queued for upload in a synced calendar), and the result is an `ImportReport`: imported, already there, skipped, adjusted.

- **No duplicates.** A row id is derived from the calendar and the event's UID (`import_id`), so the same file imported twice finds its own rows. Events the server already has are found by UID in `remote_objects`.
- **Checks before storing.** Invalid timings or rules, and rules the calendar's provider can't hold (Outlook), are reported and skipped. Extra reminders are dropped and reported.

```rust
let checked = std::iter::once(master)
    .chain(&object.overrides)
    .try_for_each(check_component);      // stops at the first Err
```

**Rust concepts:**
- **`try_for_each`:** runs a fallible check on every item and stops at the first error ([concepts §11](concepts.md#11-iterators)).
- **`flat_map` with a function name:** `objects.into_iter().flat_map(standalone)` turns objects with a lone occurrence into separate events.
- **A closure that changes a captured variable:** the `row` closure sets `dropped` when it removes reminders, so it's declared `let mut row = |…| { … }` ([§10](concepts.md#10-closures)).
- **Child modules see their parent's private items:** `import.rs` calls private `Store` helpers like `writable_calendar` and `insert` that are defined in `lib.rs`.
- **`#[derive(Serialize)]` on the report** so the desktop app can hand it straight to the UI.

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
