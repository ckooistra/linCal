# Rust concepts used in this project

Short explanations of the Rust ideas the code relies on, each with a pointer to a real example. You don't need to read this top to bottom: the crate pages link back here.

---

## 1. Variables, ownership and borrowing

Rust's core idea: **every value has exactly one owner**. When the owner goes away (at the end of a function or block), the value is freed. There's no garbage collector.

You can let other code **borrow** a value without taking ownership:

- `&T` is a shared borrow: read-only. Many can exist at once.
- `&mut T` is a mutable borrow: it can change the value, but only one can exist at a time, and no `&T` alongside it.

The compiler checks these rules. That's how Rust prevents a whole class of bugs (use-after-free, data races) without a runtime cost.

```rust
// calendar-core/src/timing.rs
pub fn resolve_local(tz: &Tz, local: NaiveDateTime) -> DateTime<Utc>
```

`tz: &Tz` borrows the time zone, because the function only needs to look at it. `local` is taken by value: `NaiveDateTime` is small and `Copy` (see §6), so a copy is passed.

**`.clone()`** makes an independent copy when you really need your own. You'll see it often where data must outlive the borrow, for example before moving it to another thread.

## 2. `String` vs `&str`

- `String` is owned, growable text (on the heap).
- `&str` is a borrowed view into some text, usually a function parameter.

The habit across this codebase: **take `&str`, return `String`**.

```rust
// calendar-ical/src/content.rs
pub fn escape_text(value: &str) -> String
```

`.to_owned()` or `.into()` turns a `&str` into a `String`.

## 3. Structs and enums

A **struct** groups named fields. An **enum** is "one of several shapes", and each shape (variant) can carry its own data. Rust enums are far more powerful than enums in C or Java. They're the main modelling tool in this project.

```rust
// calendar-core/src/model.rs
pub enum EventTiming {
    Zoned    { start: DateTime<Utc>, end: DateTime<Utc>, time_zone: Tz },
    Floating { start: NaiveDateTime,  end: NaiveDateTime },
    AllDay   { start_date: NaiveDate, end_date: NaiveDate },
}
```

An event's time is *exactly one* of these. It's impossible to build an "all-day event with a time zone" by mistake, because that shape doesn't exist.

## 4. `match` and pattern matching

`match` handles each variant of an enum. The compiler **refuses to compile** if you forget one. When a new variant is added, every place that needs updating becomes a compile error, so none is missed.

```rust
// calendar-caldav/src/client.rs
match status.as_u16() {
    401 => ProviderError::Unauthorized,
    404 | 410 => ProviderError::NotFound(url.path().to_owned()),
    429 | 503 => ProviderError::RateLimited { retry_after: … },
    s if s >= 500 => ProviderError::Server { status: s },
    s => ProviderError::InvalidResponse(format!("HTTP {s}")),
}
```

Related shortcuts you'll see:

- `if let Some(x) = value { … }` matches just one pattern.
- `let Some(x) = value else { return … };` either binds `x` or leaves the function.
- `matches!(value, Pattern)` is a pattern test that gives a `bool`.
- `if let A = a && let B = b { … }` chains several conditions ("let chains", new in Rust 2024). You'll find them in `calendar-caldav/src/lib.rs` and `calendar-sync`.

## 5. `Option` and `Result`: no null, no exceptions

- `Option<T>` is either `Some(value)` or `None`. Rust has no `null`.
- `Result<T, E>` is either `Ok(value)` or `Err(error)`. Rust has no exceptions.

Both are ordinary enums. The **`?` operator** means "if this is an error, return it from my function now; otherwise give me the value":

```rust
// apps/desktop/src-tauri/src/commands.rs
let tz = zone(&time_zone)?;
let occurrences = state.store()?.query_range(range.to_range()?, tz).await?;
```

Each `?` is a place where the function may stop early with an error.

You'll also see `.unwrap()` and `.expect("…")`, which mean "crash if this is an error". The app avoids them except in tests, and where failure really is impossible (`expect("valid")` on a hard-coded URL).

## 6. Traits: shared behaviour

A **trait** is a set of methods a type promises to have, like an interface in other languages.

**Derived traits.** `#[derive(...)]` asks the compiler to write common trait implementations for you:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar { … }
```

| Trait | Gives you |
|---|---|
| `Debug` | `{:?}` printing, for logs and test failures |
| `Clone` | `.clone()` |
| `Copy` | implicit copying for small values, with no `.clone()` needed |
| `PartialEq`, `Eq` | `==` |
| `Hash` | use as a `HashMap` key |
| `PartialOrd`, `Ord` | `<`, sorting, `BTreeSet` |
| `Default` | `Type::default()`, an "empty" value |
| `Serialize`, `Deserialize` | JSON conversion (from the **serde** library) |

**Your own traits.** `calendar-provider-api` defines `CalendarProvider`, and four provider crates implement it. That trait is the "plug-in socket" of the architecture.

**Standard traits implemented by hand.** For example, `Display` (for `{}` formatting) and `FromStr` (for `.parse()`) on `OccurrenceKey` in `calendar-core/src/occurrence_key.rs`. And `From<A> for B`, which lets `?` convert one error type into another automatically (§9).

## 7. Generics and trait objects (`dyn`)

There are two ways to write code that works with "any type that implements trait X":

- **Generics** (`fn pick<T: PartialEq + Clone>(…)`): the compiler makes a specialised copy for each type. Fast, but the type is fixed at compile time. Example: `pick` in `calendar-core/src/merge.rs`.
- **Trait objects** (`Box<dyn CalendarProvider>`, `Arc<dyn CredentialStore>`): one piece of code works with whatever type turns up at **run time**. The sync engine uses this: it doesn't know until it reads the database whether an account is CalDAV or Google.

```rust
// calendar-sync/src/engine.rs: "a function that builds some provider"
pub type ProviderFactory = Arc<
    dyn Fn(&RemoteAccount, &str) -> Result<Box<dyn CalendarProvider>, ProviderError> + Send + Sync,
>;
```

`impl Trait` in a return position (`-> impl Future<Output = ()>`) means "some type implementing this; I won't name it". See `scheduler()` in `calendar-sync/src/scheduler.rs`.

## 8. Modules and visibility

- `mod name;` includes `name.rs` as a module.
- Everything is **private by default**. `pub` makes it visible outside the module.
- `pub(crate)` means visible inside this crate only. `calendar-storage` uses it to share helpers between `lib.rs` and `remote.rs` without exposing them.
- `pub use x::Y;` re-exports `Y`, so users write `calendar_core::Calendar` rather than `calendar_core::model::Calendar`. Each `lib.rs` does this.
- `use crate::…` refers to the current crate; `super::*` refers to the parent module (used in tests).

## 9. Error handling with `thiserror`

Each crate has its own error enum (`CoreError`, `StorageError`, `ProviderError`, `SyncError`). The `thiserror` library writes the boilerplate:

```rust
// calendar-storage/src/lib.rs
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),     // `?` on an sqlx error converts automatically
    #[error("the calendar “{0}” is read-only")]
    ReadOnly(String),
    …
}
```

Errors flow upward and get **wrapped** at each layer: `sqlx::Error` → `StorageError` → `SyncError` → `CommandError`. At the very top (`desktop-app`), each is turned into a friendly message for the UI.

## 10. Closures

A closure is an inline function that can capture variables from around it: `|x| x + 1`. The `move` keyword makes it take ownership of what it captures, which is needed when the closure outlives the current function (for example, it runs on another task).

```rust
// apps/desktop/src-tauri/src/lib.rs: called for every due reminder
move |r| {
    if enabled.load(Ordering::Relaxed) {
        notify(&handle, &r);
    }
}
```

Closures are how the engine reports back to the app without knowing about Tauri: the app hands in a closure, and the engine calls it.

## 11. Iterators

Rust code often chains iterator methods instead of writing loops:

```rust
// apps/desktop/src-tauri/src/commands.rs
occurrences.iter().map(|o| WireEvent::from_occurrence(o, &tz)).collect()
```

Common ones: `map`, `filter`, `find_map`, `any`, `fold`, `collect`. They're lazy (nothing happens until `collect`, a `for` loop or similar) and compile to code as fast as a hand-written loop.

## 12. Lifetimes (`'a`)

Sometimes the compiler needs to know how long a borrowed value lives. A **lifetime** label like `'a` says "this result borrows from that input":

```rust
// calendar-core/src/sim.rs
fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T
```

This reads: "the returned reference lives as long as `items`". Most of the time the compiler works this out itself, so explicit lifetimes are rare in this codebase. `'static` means "lives for the whole program", for example string literals (`&'static str`).

## 13. Smart pointers and sharing: `Box`, `Arc`, `Mutex`

- **`Box<T>`:** a value on the heap with one owner. Used for trait objects (`Box<dyn CalendarProvider>`).
- **`Arc<T>`:** shared ownership. Several parts of the program hold the same value, and it's freed when the last one lets go. It's safe across threads ("Atomic Reference Counted"). `Arc::clone(&x)` is cheap: it copies a pointer, not the data.
- **`Mutex<T>`:** lets only one thread at a time *change* the value inside. `.lock()` gives temporary access.
- **`RwLock<T>`:** many readers *or* one writer.
- **`AtomicBool`:** a single true/false that threads can share without a lock. Used for the "notifications on/off" setting.

The usual pattern is `Arc<Mutex<T>>`: shared *and* changeable. `std::sync::Mutex` is for quick, non-async access. `tokio::sync::Mutex` can be held across an `.await` (§14).

**`Send` and `Sync`** are marker traits the compiler checks. `Send` means "can move to another thread"; `Sync` means "can be shared between threads". You'll see `+ Send + Sync` on trait objects that the engine passes between tasks.

## 14. `async`/`.await` and Tokio

Calendar work spends most of its time **waiting** for the network or the disk. `async` lets one thread juggle many waiting jobs:

- `async fn` returns a **future**: a job that hasn't run yet.
- `.await` means "run this, and let other jobs use the thread while it waits".
- **Tokio** is the runtime that schedules futures on a few threads.

Tools from Tokio used here:

| Tool | Meaning | Where |
|---|---|---|
| `tokio::spawn` | run a future in the background as its own **task** | `scheduler.rs` |
| `JoinSet` | run several tasks and wait for each | `SyncEngine::sync_all` |
| `Semaphore` | allow at most N at a time | 3 accounts syncing at once; 2 expansions at once |
| `Notify` | wake a sleeping task | "Sync now", reminders |
| `select!` | wait for whichever finishes first | scheduler: timer *or* wake-up |
| `timeout` | give up after a while | 10-minute cap on a sync pass |
| `spawn_blocking` | run CPU-heavy or blocking code on a separate thread pool | recurrence expansion, keyring |

**The golden rule:** never do slow CPU work or blocking calls directly inside `async` code, because that stalls every task sharing the thread. `spawn_blocking` is the escape hatch, and this project uses it deliberately. That's how "the UI never freezes" is achieved.

**`#[async_trait]`:** traits with `async fn` that are used as `dyn` objects need this helper macro.

## 15. Macros

Anything ending in `!` is a **macro**: code that writes code at compile time.

- `println!`, `eprintln!`, `format!`: formatted text. `eprintln!` writes to the error log.
- `vec![…]`: build a `Vec`.
- `assert!`, `assert_eq!`: checks in tests.
- `sqlx::migrate!("./migrations")`: embeds the SQL migration files into the program.
- `tauri::generate_handler![…]`: registers the commands the UI may call.

`#[…]` above an item is an **attribute**, for example `#[derive]`, `#[test]`, `#[tauri::command]` or `#[serde(rename_all = "camelCase")]`.

## 16. Features and conditional compilation

`Cargo.toml` can define optional **features**. Code marked `#[cfg(feature = "x")]` only exists when that feature is on. In this project:

- `calendar-core` has the `sim` feature: the fake dataset generator, used by benchmarks only.
- `calendar-sync` has `keyring`: the real OS keyring. Tests run without it.
- `calendar-google` and `calendar-microsoft` have `testing`: fake API servers for tests.

`#[cfg(test)]` code only exists when running `cargo test`.

## 17. Tests

- **Unit tests** live in the same file, inside `#[cfg(test)] mod tests { … }`. They can test private functions.
- **Integration tests** live in a crate's `tests/` folder and use only the public API.
- `#[test]` marks a normal test; `#[tokio::test]` marks an async one.
- **wiremock** starts a fake HTTP server, so provider tests never touch the real internet.

Run them with `cargo test -p <crate>`.

## 18. Numbers and small details

- Integer types state their size: `i64`, `u32`, `usize` (pointer-sized, used for lengths).
- Conversions are explicit: `u64::try_from(ms)` fails instead of silently wrapping.
- `wrapping_add` and `wrapping_mul` overflow on purpose. They're used in hash functions (`sim.rs`, `calendar-ics`), where wrapping is wanted.
- `const` is a compile-time constant. `static` is a single global value (`MIGRATOR` in `calendar-storage`).
- `Cow<'a, str>` ("clone on write") is either borrowed or owned text. It avoids a copy when nothing needs changing (`until_in_utc` in `recurrence.rs`).
