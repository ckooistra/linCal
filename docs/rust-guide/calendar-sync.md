# calendar-sync

**Where:** `crates/calendar-sync/`
**Depends on:** `calendar-storage`, `calendar-provider-api`, all four provider crates, `calendar-oauth`, `calendar-core`, `tokio`, and `keyring` (optional).
**Used by:** the desktop app.

## Its job in the architecture

The **background worker** that keeps the local database and the servers in step. Everything here runs on Tokio tasks, so the UI never waits for it.

```text
 local edit ─► Store (saved + queued) ─► scheduler.trigger()
                                              │  (waits 1.5 s to batch edits)
                                              ▼
                        SyncEngine::sync_all ── up to 3 accounts in parallel
                                              │
                    for each account: sync_account → pass()
                       1. upload queued changes   (put / delete with version checks)
                       2. refresh calendar list   (new, renamed, removed, read-only)
                       3. download changes        (since the last cursor)
                                              │
                     listener(SyncEvent) ─► desktop app ─► UI refreshes
```

Design points (from `CLAUDE.md`):

- **Upload before download,** so a server change never silently replaces an edit that's still queued.
- **Accounts are isolated.** One slow, failing or even crashing account doesn't stop the others.
- **No busy polling.** Tasks sleep until there's a reason to wake.
- **Failures back off:** 30 s, doubling up to 1 h.
- **Conflicts are recorded, never overwritten.** A three-way merge is tried first.

## Files

### `lib.rs`

Module list and re-exports. One line is worth noticing:

```rust
pub use tokio::sync::Notify as CancelSignal;
```

This re-exports another library's type under a name that makes sense to callers.

### `engine.rs`: `SyncEngine` (the biggest piece)

```rust
pub struct SyncEngine {
    store: Store,
    credentials: Arc<dyn CredentialStore>,
    providers: ProviderFactory,          // builds a provider for an account
    listener: Listener,                  // reports progress to whoever is listening
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,  // one pass per account
    backoff: Mutex<HashMap<String, Backoff>>,
}
```

Main parts:

- **Adding accounts:** `add_caldav_account` (with discovery), `add_ics_subscription`, `add_google_account` and `add_microsoft_account` (browser sign-in), and `sign_in_again`. Each can be cancelled.
- **`sync_all(forced)`** starts one task per account, at most 3 at a time:

  ```rust
  let permits = Arc::new(Semaphore::new(MAX_PARALLEL_ACCOUNTS));
  let mut tasks = tokio::task::JoinSet::new();
  for account in self.store.remote_accounts().await? {
      let engine = Arc::clone(self);
      tasks.spawn(async move {
          let _permit = permits.acquire().await;
          let _ = engine.sync_account(&id).await;
      });
  }
  while let Some(done) = tasks.join_next().await {
      if let Err(e) = done && e.is_panic() { eprintln!("[sync] an account sync task panicked"); }
  }
  ```

  A panic (crash) inside one account's task is caught here, and the other accounts carry on.
- **`sync_account`** takes that account's lock, runs the pass with a 10-minute `timeout`, records success or failure, and tells the listener.
- **`pass`** runs the three steps. Parsing downloaded data is CPU work on untrusted input, so it runs in `spawn_blocking`.
- **`push` / `upload` / `merge_with_server`:** on `PreconditionFailed`, the engine fetches the server's version and tries `calendar_core::merge::three_way`. If both sides changed the same thing, the server's copy is kept for the comparison dialog and the event is marked as a conflict.
- **`conflict_view`, `problems`, `resolve`** feed the "Review…" dialog.
- **`user_message`** turns each error into one friendly sentence.
- **`default_factory`** matches on `ProviderKind` and builds the right provider from the stored config and secret:

  ```rust
  ProviderKind::Caldav => {
      let config: CalDavConfig = serde_json::from_value(account.config.clone())?;
      Ok(Box::new(CalDavProvider::new(home, &config.username, secret)?) as Box<dyn CalendarProvider>)
  }
  ```

**Rust concepts:**
- **Trait objects everywhere:** the engine holds `Arc<dyn CredentialStore>` and builds `Box<dyn CalendarProvider>`. It never names a concrete provider outside `default_factory` ([concepts §7](concepts.md#7-generics-and-trait-objects-dyn)).
- **Closures stored as fields:** `ProviderFactory` and `Listener` are `Arc<dyn Fn(…) + Send + Sync>`. The app passes in "what to do on each event" without the engine knowing about Tauri ([§10](concepts.md#10-closures)).
- **`self: &Arc<Self>`:** `sync_all` needs to hand clones of the engine to new tasks, so it asks to be called on an `Arc` ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **`JoinSet`, `Semaphore`, `timeout`, `spawn_blocking`** ([§14](concepts.md#14-asyncawait-and-tokio)).
- **A lock per account:** `HashMap<String, Arc<tokio::sync::Mutex<()>>>`. The `()` means the mutex guards nothing but *time*: only one pass per account runs at once.
- **Nested results:** `Result<Result<(), ProviderError>, SyncError>` separates "our database failed" (outer) from "the server said no" (inner), which call for different reactions.
- **`#[serde(tag = "type")]` on `SyncEvent`:** the JSON comes out as `{"type": "synced", "accountId": …}`, easy to handle in TypeScript.
- **`#[from]` in `SyncError`:** `?` turns `ProviderError`, `StorageError` and `OAuthError` into `SyncError` automatically ([§9](concepts.md#9-error-handling-with-thiserror)).

### `scheduler.rs`: when to sync

Runs passes at start-up, when asked (a local edit or "Sync now"), every 5 minutes, and when a queued retry falls due. In between it **sleeps**, using no CPU.

```rust
forced = tokio::select! {
    () = tokio::time::sleep(wait) => false,          // timer: respect backoff
    () = wake.notified() => {                        // someone asked
        tokio::time::sleep(DEBOUNCE).await;          // gather a burst of edits into one pass
        true
    }
};
```

`scheduler()` returns two things: a **handle** (cheap to clone, with `trigger()`) and the **loop future**, which the caller spawns. The engine doesn't decide which runtime it runs on; the app does.

**Rust concepts:** `select!` ([§14](concepts.md#14-asyncawait-and-tokio)); returning a tuple of `(Handle, impl Future)`; each pass runs as its own `tokio::spawn`, so a panic ends that pass, not the scheduler.

### `reminders.rs`: desktop notifications

The same pattern as the scheduler: a loop that sleeps **until the next reminder** (at most an hour), or until `RemindersHandle::changed()` says events changed.

- `due_reminders(store, after, until, …)` asks storage for occurrences and returns the reminders that fire in the window `(after, until]`. Using half-open windows means no reminder fires twice and none is skipped if the loop wakes late.
- After the computer sleeps, missed reminders show only for events that haven't ended.
- `show` is a closure from the app. The engine doesn't know how notifications are displayed.

**Rust concepts:**
- **`flat_map`:** each occurrence yields several reminders, flattened into one list ([§11](concepts.md#11-iterators)).
- **`sort_by_key(|r| (r.fire_at, r.start))`:** tuples sort field by field.
- **`impl Fn() -> Tz + Send + 'static`** as a parameter type: "any closure that returns a time zone and can move to another thread" ([§10](concepts.md#10-closures)).

### `import.rs`: importing an .ics file

`import_ics(store, calendar_id, text)` is the whole "Import events from a file" feature, minus the UI:

1. It refuses files over 20 MB.
2. It splits the file into one object per UID with `calendar_ics::split_feed` (the same code subscriptions use), and reads each with `ICalendarCodec`. Both steps run on the blocking pool, because the text is untrusted and may be large.
3. It hands the objects to `Store::import_objects`, then adds any events that couldn't be read to the report.

It takes a `&Store` rather than the engine, because importing into a local calendar needs no network. Uploading happens later, through the normal sync pass.

**Rust concepts:**
- **A dedicated error enum** (`ImportError`) with `#[error(transparent)]`, which shows the wrapped storage error's message unchanged ([§9](concepts.md#9-error-handling-with-thiserror)).
- **`move` into `spawn_blocking`:** the `String` is moved into the closure, so the thread owns it and no copy is made ([§1](concepts.md#1-variables-ownership-and-borrowing)).
- **Unit structs as values:** `ICalendarCodec.read(…)` calls a method on a struct with no fields.

### `credentials.rs`: where passwords live

```rust
pub trait CredentialStore: Send + Sync {
    fn get(&self, account_id: &str) -> Result<Option<String>, String>;
    fn set(&self, account_id: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, account_id: &str) -> Result<(), String>;
}
```

There are two implementations:

- **`KeyringCredentials`:** the OS keyring (KWallet on Plasma, GNOME Keyring, the macOS Keychain, the Windows Credential Manager). Only compiled with the `keyring` feature.
- **`MemoryCredentials`:** a `Mutex<HashMap>`, for tests (and `UC_CREDENTIALS=memory`).

Keyring calls can block, so the engine always calls them inside `spawn_blocking`.

**Rust concepts:**
- **A trait with two implementations,** chosen at run time ([§6](concepts.md#6-traits-shared-behaviour)).
- **`#[cfg(feature = "keyring")]`** on the type, its `impl` and its re-export ([§16](concepts.md#16-features-and-conditional-compilation)).
- **`#[derive(Default)]`**, so `MemoryCredentials::default()` gives an empty store.
- **Or-patterns in `match`:** `Ok(()) | Err(keyring::Error::NoEntry) => Ok(())`, because "already deleted" counts as success.

## Tests (`tests/`)

| File | What it covers |
|---|---|
| `engine.rs` | the CalDAV engine against wiremock: upload, download, conflicts, merges, retries, backoff |
| `google.rs`, `microsoft.rs` | the full engine with `FakeGoogle` and `FakeGraph` |
| `ics.rs` | subscriptions end to end |
| `engine.rs` (`imported_files_are_uploaded_once`) | an imported series with an exception uploaded once, then skipped on re-import |
| `real_server.rs` | optional, against a real CalDAV server (Radicale or Nextcloud), when `UC_CALDAV_URL` is set |

Each test builds its own engine with an in-memory `Store` and `MemoryCredentials`. Trait objects make this swap easy: the engine can't tell a fake from the real thing.
