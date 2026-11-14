# ucal: the terminal front end (`apps/ucal`)

**Where:** `apps/ucal/`
**Depends on:** `calendar-core`, `calendar-storage`, `calendar-sync` (with the `keyring` feature), `clap`, `ratatui`, `tokio`, `iana-time-zone`.
**Used by:** nothing. This is a program you run (`ucal`).

## Its job in the architecture

A **second front end** next to the desktop app, and the proof that the engine really is independent of Tauri. `ucal` opens the same database, builds the same `SyncEngine`, and calls the same `Store` methods. It has two faces: **commands** that print and exit (`ucal agenda`, `ucal add …`), and a **full-screen view** (`ucal` alone, in `src/tui/`).

```text
   desktop window            terminal
        │                       │
 apps/desktop/src-tauri     apps/ucal          ← two thin front ends
        └──────────┬────────────┘
     calendar-storage · calendar-sync          ← one engine, one database
```

Like the Tauri crate, it holds no calendar logic. Each command reads its arguments, makes a few calls into the engine, and prints the result. The one new piece of logic it needed, reading "fri 2:30pm" and "1h30m", went into `calendar-core/src/input.rs`, where any front end can use it.

Two programs sharing one database raises one real problem: both could sync the same account at the same time. `calendar-sync/src/process_lock.rs` solves it with an OS file lock (see [calendar-sync.md](calendar-sync.md#process_lockrs-one-program-syncs-an-account-at-a-time)).

## Files

### `main.rs`: arguments and commands

The command line is described as Rust types, and `clap` generates the parsing, the `--help` text and the error messages from them:

```rust
#[derive(Subcommand)]
enum Command {
    /// Upcoming events, day by day (what `ucal` alone shows)
    Agenda {
        #[arg(default_value = "today")]
        date: String,
        #[arg(short, long, default_value_t = 7, value_parser = clap::value_parser!(u16).range(1..=366))]
        days: u16,
    },
    Add { title: String, date: String, time: Option<String>, /* … */ },
    // Search, Import, Calendars, Sync
}
```

The `///` comments become the help text, and `Option<String>` makes an argument optional. `run` then has one `match` over the commands.

`App::open` does what the desktop app's `setup` does:
- finds the data directory (`UC_DATA_DIR`, or the same folder Tauri uses);
- opens the `Store`;
- builds a `SyncEngine` whose listener sends each `SyncEvent` into a channel. `ucal sync` reads the channel afterwards and prints `✓ Work` or `✗ Work: …`; the full-screen view reads it while it runs.

It builds the engine even for `ucal add`, because creating the engine also tells storage what each provider can hold (Outlook's single reminder, for example).

`main` creates a **single-threaded** Tokio runtime and runs the async `run` function to completion with `block_on`. A short command-line program doesn't need a thread per CPU.

**Rust concepts:**
- **Derive macros generating a parser** (`#[derive(Parser)]`, `#[derive(Subcommand)]`) ([concepts §15](concepts.md#15-macros)).
- **`Box<dyn Error>`** as a catch-all error type: `?` turns any error (storage, import, parsing) into it, and a `String` converts with `.into()`. That's fine for a program that only prints errors; a library would use a `thiserror` enum ([§9](concepts.md#9-error-handling-with-thiserror)).
- **`ExitCode`:** `main` returns success or failure to the shell, so scripts can test `ucal sync`.
- **`let … else`:** `let Some(time) = time else { /* all-day event */ };` handles the "no time given" case first and returns early.
- **Channels:** `tokio::sync::mpsc::unbounded_channel()` gives a sender, moved into the listener closure, and a receiver kept in `App`. The engine's tasks send; whoever runs the command receives, in order ([§14](concepts.md#14-asyncawait-and-tokio)).
- **`cfg!(windows)`:** a compile-time condition used like a normal `bool` ([§16](concepts.md#16-features-and-conditional-compilation)).
- **`std::io::IsTerminal`:** colours only when printing to a terminal, never into a file or pipe.

### `show.rs`: text output

Pure functions that return `String`s: the tests compare the exact text.

- `day_label` decides what an event shows on one day. An event over several days appears on each: `22:00 →`, then `all day`, then `→ 06:30`.
- `days` prints a heading per day and one line per event, all-day ones first.
- `attention` turns an account's status into notes such as "⚠ 1 conflict to review in the desktop app." Problems are never hidden.
- `Style` adds ANSI colour codes (a dot in each calendar's colour), or nothing.

**Rust concepts:**
- **`Copy` types:** `Style` is a one-field struct marked `#[derive(Clone, Copy)]`, so it is passed by value like a number.
- **Matching on a tuple of `bool`s:** `match (starts_today, ends_today)` covers the four cases in one place, and the compiler checks none is missing ([§4](concepts.md#4-match-and-pattern-matching)).
- **`bool::then`:** `(start <= day && day < end).then(|| "all day".to_owned())` gives `Some(…)` or `None`.
- **`std::fmt::Write`:** `writeln!(out, …)` writes into a `String` the way `println!` writes to the screen.

### `calendars.rs`: finding a calendar by name

`find` accepts an id, a name ignoring case, or `Account/Name` when two accounts both have a "Tasks". If a name is ambiguous, the error lists the full names to choose from. `default` picks the desktop app's default calendar if it can still take events.

**Rust concepts:** **slice patterns** in `match by_name.as_slice() { [one] => …, [] => …, several => … }` match on how many items a list has. Also lifetimes: `find<'a>(…, calendars: &'a [Calendar], …) -> Result<&'a Calendar, String>` returns a reference *into* the list it was given ([§12](concepts.md#12-lifetimes-a)).

### `tui/`: the full-screen view

Three files, each with one job. This is the "Elm architecture", common in Rust terminal apps:

```text
 keys (thread) ─┐
 query results ─┼─► Msg ─► State::update ─► Effects ─► background tasks ─┐
 sync reports  ─┘     ▲                                                   │
                      └────────────────────── results as Msgs ◄───────────┘
          after each Msg: draw(&State)
```

- **`state.rs`:** `State` is everything on screen: view, selected day, loaded events, the form, the search. `update(msg) -> Vec<Effect>` changes it and says what work to start: `Load` a date range, `Create` an event, `Search`, `SyncNow`… It does no I/O, so every key is tested without a terminal or database.
  - **Stale answers are dropped:** each load gets a `generation` number, and a result whose number isn't the newest is ignored. Paging quickly through months never shows an old month's events.
  - **Bounded prefetch:** a view loads its own days plus as many on each side, so the next step is usually instant.
  - **Editing and deleting:** `e` opens the form filled in with the event; `x` asks before deleting. For a repeating event, an `Overlay::Choose` asks which occurrences, and the answer becomes a `Scope` (`Occurrence`, `Following`, `Series`, or `Event` when it doesn't repeat). `mod.rs` turns each scope into the matching `Store` call (`update_occurrence`, `update_following`, `delete_series`, …), as the desktop app's commands do.
  - **Changing only what was changed:** the form remembers the date, time and length it started with. If they're untouched, the event keeps its original timing, so a floating time or an event in another time zone isn't converted by renaming it. The description, reminders and repeat rule, which the form doesn't show, are passed on unchanged.
- **`draw.rs`:** turns `State` into ratatui widgets: a grid of bordered cells for the month, seven columns for the week, `List`s for the day and agenda, centred popups for details, help and the form. It reads the state and changes nothing.
- **`mod.rs`:** the loop. A thread waits for keys (`crossterm::event::read`, which uses no CPU while waiting) and sends them into the same channel as query results and sync reports. The loop draws, waits for a message, calls `update`, and starts each effect as a `tokio::spawn`ed task. The sync **scheduler** runs in the background as in the desktop app. `ratatui::init()` switches the terminal to full screen and installs a panic hook that puts it back; `ratatui::restore()` puts it back on exit.

**Rust concepts:**
- **Enums carrying data as messages:** `Msg::Loaded { generation, range, result }`. One `match` in `update` handles every kind of event, and the compiler insists that each one is handled ([§3](concepts.md#3-structs-and-enums), [§4](concepts.md#4-match-and-pattern-matching)).
- **Separating decisions from actions:** `update` returns `Vec<Effect>` instead of doing the work, which is what makes it testable.
- **Destructuring a struct:** `let App { store, engine, tz, .. } = app;` takes the parts out of `App` so each can move where it's needed.
- **`let … else` in tests:** `let [Effect::Create(draft)] = effects.as_slice() else { panic!() };` checks that exactly one effect came back, and names it.
- **`std::mem::replace`:** `let Overlay::Form(form) = std::mem::replace(&mut self.overlay, Overlay::None)` takes the form *out* of the state (leaving `None` behind) so it can be moved into the choice popup, and moved back if the choice is cancelled. Rust won't let you move a value out of a `&mut` reference without leaving something in its place ([§1](concepts.md#1-variables-ownership-and-borrowing)).
- **Matching on a tuple of enums:** `match (action, scope)` with `(Action::Save(saved), None)`, `(Action::Delete, Some(scope))`… handles every combination in one place ([§4](concepts.md#4-match-and-pattern-matching)).
- **`Box` to keep an enum small:** `Msg::Created(Result<Box<CalendarEvent>, String>)`. Without the box every `Msg` would be as big as the largest variant (clippy points this out).
- **Threads and channels:** a `std::thread` sends into a Tokio channel; `UnboundedSender::send` works from any thread ([§14](concepts.md#14-asyncawait-and-tokio)).

## Tests

- `edits_and_deletions_reach_the_database` in `tui/mod.rs` runs each scope of change and deletion through the event loop's own code on a real (in-memory) database, including splitting a series.
- Unit tests in `show.rs`, `calendars.rs`, `tui/state.rs` (keys, loading, the form, editing and deleting, search, sync reports) and `tui/draw.rs` (each view drawn on ratatui's `TestBackend`, an in-memory screen whose text the test reads): `cargo test -p ucal`.
- A drawing benchmark with the 100/1,000/5,000-event datasets: `cargo test --release -p ucal -- --ignored --nocapture drawing_time` (results in `docs/performance.md`).
- The date and time parsing is tested in `calendar-core` (`cargo test -p calendar-core input`).
- `two_programs_never_sync_the_same_account_at_once` in `calendar-sync/tests/engine.rs` opens one database file twice, with two engines, and checks that the second pass waits and uploads nothing twice.

## Running it

```sh
cargo run -p ucal                    # the full-screen view, from the checkout
cargo run -p ucal -- agenda          # a command
cargo install --path apps/ucal       # installs ~/.cargo/bin/ucal
UC_DATA_DIR=/tmp/try ucal add "Test" today 15:00   # a throwaway database
```
