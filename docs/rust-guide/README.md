# A Rust guide to Universal Calendar

This guide walks through every Rust file in the project. For each one it covers what the file does, where it fits in the architecture, and which Rust ideas it shows. It's written for someone who is new to Rust.

- **[concepts.md](concepts.md)** explains the Rust ideas used in this project in plain terms, with links to where each one appears. Read it first, or keep it open alongside the other pages.
- The other pages cover one **crate** each (a crate is a Rust library or program; see below).

## The big picture

Universal Calendar is a desktop app. The window is a web page (Svelte and TypeScript) shown by **Tauri**. Everything else, from storing events to talking to Nextcloud or Google, is Rust.

```text
 ┌──────────────────────────────────────────────┐
 │  Svelte UI (TypeScript)                      │  what you see and click
 └───────────────▲──────────────────────────────┘
                 │ invoke("query_range", …) / events
 ┌───────────────┴──────────────────────────────┐
 │  apps/desktop/src-tauri  (thin bridge)       │  desktop-app.md
 └───────▲──────────────────────────▲───────────┘
         │                          │
 ┌───────┴─────────┐      ┌─────────┴──────────────────────────┐
 │ calendar-storage│◄─────┤ calendar-sync                       │
 │ SQLite database │      │ background engine, scheduler,       │
 │ (local-first)   │      │ reminders, keyring                  │
 └───────▲─────────┘      └─────────▲──────────────────────────┘
         │                          │ uses the CalendarProvider trait
         │              ┌───────────┴─────────────────────────────┐
         │              │ calendar-provider-api (the contract)     │
         │              └──▲─────────▲──────────▲──────────▲───────┘
         │                 │         │          │          │
         │            caldav     google    microsoft      ics
         │                 │         │  (oauth) │          │
         │                 └─────────┴─── calendar-ical ───┘
         │                                  │
 ┌───────┴──────────────────────────────────┴──────────────┐
 │ calendar-core: models, time zones, recurrence, merge    │
 │ (no I/O, no database, no network, no Tauri)             │
 └─────────────────────────────────────────────────────────┘
```

Arrows point from a crate to the crates it uses. The lower a crate sits, the fewer other crates it knows about.

### Why it's split this way

These rules come from `CLAUDE.md`, and Rust's crate system enforces them:

1. **The calendar engine doesn't know about Tauri.** Only `apps/desktop/src-tauri` depends on Tauri, and it holds no calendar logic. The engine could run in a different shell (a CLI, another GUI toolkit) unchanged.
2. **Providers are plug-ins.** CalDAV, Google, Microsoft and ICS each implement one trait (`CalendarProvider`). The sync engine only sees that trait, never "Google".
3. **Local-first.** The UI only ever reads from SQLite (`calendar-storage`). Network work happens in the background (`calendar-sync`), and its results land in SQLite.
4. **The UI is never blocked.** Everything that waits (disk, network) is `async`. Heavy CPU work, such as expanding recurring events, runs on a separate thread pool.

Because each crate lists its dependencies in its own `Cargo.toml`, a rule like "core must not touch the network" isn't just a convention. `calendar-core` has no HTTP library in its dependencies, so it *can't* make network calls.

## Two journeys through the code

**1. You open the month view.**
`App.svelte` calls `invoke("query_range")`. That runs `commands::query_range` in `desktop-app`, which calls `Store::query_range` in `calendar-storage`. SQL finds candidate rows; `recurrence::expand` in `calendar-core` turns each recurring series into the occurrences inside the month. The results become `WireEvent`s (`calendar-core/wire.rs`) and return to the UI as JSON.

**2. You move an event in a Nextcloud calendar.**
`commands::update_event` calls `Store::update_event`. In one transaction this saves the event and queues an upload. The UI updates at once. `AppState::changed` then wakes the **scheduler** (`calendar-sync`), which runs `SyncEngine::sync_account`. That builds the iCalendar text (`calendar-ical`) and calls `CalDavProvider::put_object`. If the server says someone else changed it first, `merge::three_way` (`calendar-core`) tries to combine both edits. If they clash, the event is marked as a conflict for you to resolve.

## Reading order

Start at the bottom of the diagram and work up:

| # | Page | Crate(s) | You'll learn |
|---|---|---|---|
| 1 | [calendar-core.md](calendar-core.md) | `calendar-core` | structs, enums, `match`, `Option`/`Result`, traits, tests |
| 2 | [calendar-ical.md](calendar-ical.md) | `calendar-ical` | strings, parsing, iterators, lifetimes |
| 3 | [calendar-storage.md](calendar-storage.md) | `calendar-storage` | `async`, SQLx, transactions, `Arc`, threads |
| 4 | [calendar-provider-api.md](calendar-provider-api.md) | `calendar-provider-api` | traits as contracts, trait objects (`dyn`) |
| 5 | [calendar-caldav.md](calendar-caldav.md) | `calendar-caldav` | HTTP with reqwest, XML, error mapping |
| 6 | [calendar-oauth.md](calendar-oauth.md) | `calendar-oauth` | async networking, `Mutex`, security habits |
| 7 | [calendar-google.md](calendar-google.md) | `calendar-google` | JSON with serde, test doubles |
| 8 | [calendar-microsoft.md](calendar-microsoft.md) | `calendar-microsoft` | translating between formats |
| 9 | [calendar-ics.md](calendar-ics.md) | `calendar-ics` | reuse, builder methods, hashing |
| 10 | [calendar-sync.md](calendar-sync.md) | `calendar-sync` | Tokio tasks, `select!`, closures, isolation |
| 11 | [desktop-app.md](desktop-app.md) | `apps/desktop/src-tauri` | Tauri commands, shared state, `From` |

## Words you'll see everywhere

- **Crate:** one compilable unit, either a library or a program. Each folder under `crates/` is a crate with its own `Cargo.toml`.
- **Workspace:** the top-level `Cargo.toml` that groups all crates so they build together and share settings (edition, licence, minimum Rust version).
- **Module:** a file (or folder) inside a crate. `pub mod recurrence;` in `lib.rs` pulls in `recurrence.rs`.
- **`lib.rs`:** the entry point of a library crate. It decides what the outside world may use (`pub`).
- **`tests/` folder:** integration tests that use the crate from outside, like any other user of it.
- **`examples/` folder:** small runnable programs (`cargo run --example name`).

## Trying things yourself

```sh
cargo test -p calendar-core              # run one crate's tests
cargo test -p calendar-core recurrence   # only tests whose name contains "recurrence"
cargo doc -p calendar-core --open        # browse the API docs built from /// comments
cargo clippy --workspace                 # Rust's linter, which explains many idioms
```

A good exercise: pick a test in `calendar-core/src/timing.rs`, change an expected value, and read the failure message.
