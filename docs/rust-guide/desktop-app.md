# The desktop app (`apps/desktop/src-tauri`)

**Where:** `apps/desktop/src-tauri/`
**Depends on:** `tauri` and its plugins (opener, notification), `calendar-core`, `calendar-storage`, `calendar-sync` (with the `keyring` feature), `calendar-provider-api`.
**Used by:** nothing. This is the program you run.

## Its job in the architecture

The **thin bridge** between the Svelte UI and the Rust engine. `CLAUDE.md`'s first architecture rule is "Tauri is a thin command/event bridge; do not put calendar logic in Tauri commands", and this crate follows it. Each command:

1. decodes the arguments from the UI (JSON → Rust types, via `wire.rs`);
2. makes **one** call into `calendar-storage` or `calendar-sync`;
3. turns the result or error back into JSON.

If the app ever moved to another GUI toolkit, only this folder (and the UI) would change.

Talking to the UI goes both ways:

- **UI → Rust:** `invoke("query_range", {…})` in TypeScript runs a `#[tauri::command]` function.
- **Rust → UI:** `handle.emit("sync-event", event)` pushes sync progress to the UI, which listens for it.

## Files

### `main.rs`: the entry point

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() { universal_calendar_desktop_lib::run() }
```

It does almost nothing on purpose. The real code is in the library (`lib.rs`), so it can also be used from tests and mobile entry points. The attribute hides the console window on Windows release builds.

**Rust concepts:** `#![…]` (with `!`) is an attribute for the *whole file*. `cfg_attr` applies an attribute only under a condition ([concepts §15](concepts.md#15-macros)).

### `build.rs`: a build script

```rust
fn main() { tauri_build::build() }
```

Cargo runs `build.rs` **before** compiling the crate. Tauri uses it to read `tauri.conf.json`, the icons and the permissions, and generate code from them.

**Rust concepts:** build scripts, which are ordinary Rust programs that run at compile time.

### `lib.rs`: starting everything

`run()` builds the Tauri app and, in `setup`:

1. opens the database (`open_store`), in the data directory or `UC_DATA_DIR`;
2. starts the **reminder loop**, passing a closure that shows a desktop notification;
3. creates the **`SyncEngine`**, passing the keyring (`credentials()`), the provider factory and a **listener closure** that forwards each `SyncEvent` to the UI with `emit`;
4. starts the **scheduler** on Tauri's async runtime;
5. stores everything in `AppState` with `app.manage(…)`, so commands can reach it;
6. registers every command with `tauri::generate_handler![…]`.

```rust
let engine = Arc::new(SyncEngine::new(
    store.clone(),
    credentials(),
    default_factory(),
    Arc::new(move |event| {                       // the listener
        if let (SyncEvent::Synced { changed: true, .. }, Some(wake)) = (&event, &wake) {
            wake.changed();                       // events changed: recheck reminders
        }
        let _ = handle.emit("sync-event", event); // tell the UI
    }),
));
```

This is where **dependency injection** happens: the engine gets the real keyring, real providers and a Tauri-aware listener. Tests give it fakes instead.

Other helpers: `system_zone()` reads the computer's time zone; `notify()` formats "In 15 min, 10:30 · Room 3" and shows it.

**Rust concepts:**
- **`move` closures** capturing `handle` and `wake` ([§10](concepts.md#10-closures)).
- **Matching on a tuple** of two values at once: `if let (A, Some(b)) = (&x, &y)` ([§4](concepts.md#4-match-and-pattern-matching)).
- **`Arc<AtomicBool>`** for the notifications on/off switch, shared between the reminder closure and the `set_setting` command without a lock ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **`block_on`** runs an async call to completion from non-async code. That's OK here, at start-up before the window shows, and nowhere else.
- **`Result<Store, String>` kept in state:** if the database can't open, the app still starts and the UI explains why, rather than crashing.

### `commands.rs`: what the UI may call

About 25 small `#[tauri::command]` functions: `query_range`, `search_events`, `create_event`, `update_event`, `delete_event`, `set_setting`, `add_caldav_account`, `add_google_account`, `sync_now`, `resolve_conflict`, and more.

```rust
#[tauri::command]
pub async fn query_range(state: State<'_, AppState>, range: WireRange, time_zone: String)
    -> CommandResult<Vec<WireEvent>>
{
    let tz = zone(&time_zone)?;
    let occurrences = state.store()?.query_range(range.to_range()?, tz).await?;
    Ok(occurrences.iter().map(|o| WireEvent::from_occurrence(o, &tz)).collect())
}
```

**Errors for the UI.** `CommandError { code, message }` is what TypeScript receives. `impl From<StorageError> for CommandError` (and the same for `SyncError` and `CoreError`) decides the code (`"read-only"`, `"offline"`, …) and a readable message. Technical details go to the log, not the screen. Thanks to `From`, every `?` above converts automatically.

**After a change,** `AppState::changed` wakes the reminder loop and, if the event belongs to a synced calendar, triggers the scheduler. That's the moment a local edit starts its trip to the server.

**Rust concepts:**
- **Attribute macros:** `#[tauri::command]` generates the code that turns JSON arguments into Rust values ([§15](concepts.md#15-macros)).
- **`State<'_, AppState>`:** Tauri lends each command the shared state; `'_` lets the compiler infer how long the borrow lasts ([§12](concepts.md#12-lifetimes-a)).
- **`From` implementations powering `?`** ([§9](concepts.md#9-error-handling-with-thiserror)).
- **`Deserialize` enums from the UI:** `Scope` (`"event"`, `"occurrence"`, `"series"`, `"following"`) arrives as a string and becomes an enum. An unknown value is rejected before any code runs.
- **`std::sync::Mutex<Option<Arc<CancelSignal>>>`** holds "the account setup in progress", so `cancel_add_account` can stop it.

### `bench.rs`: benchmark hooks

Used only when the person running the app sets `UC_BENCH=1` (from `scripts/bench.sh`). It provides the benchmark settings, memory readings (from `/proc`, including WebKit's helper processes) and writing the final report. The UI can't turn benchmark mode on by itself.

**Rust concepts:** reading environment variables (`std::env::var`); `Option<MemorySample>`, because memory readings aren't available on every OS; `#[derive(Serialize)]` for the report.

## Running it

```sh
cd apps/desktop
npm run tauri dev       # development, with hot reload for the UI
npx tauri build         # release build plus deb, rpm and AppImage (see docs/packaging.md)
```
