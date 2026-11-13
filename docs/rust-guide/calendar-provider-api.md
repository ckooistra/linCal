# calendar-provider-api

**Where:** `crates/calendar-provider-api/src/lib.rs` (one file)
**Depends on:** `calendar-core`, `async-trait`, `thiserror`.
**Used by:** every provider crate (they implement it) and `calendar-sync` (which calls it).

## Its job in the architecture

This small crate is the **contract** between the sync engine and the calendar services. It answers: "what must a calendar service be able to do for us to sync with it?"

```text
               calendar-sync
                    │  only knows the trait
                    ▼
        trait CalendarProvider  ◄── this crate
          ▲      ▲       ▲      ▲
       CalDAV  Google  Graph   ICS      each implements it
```

Because the engine depends only on the trait, adding a new service (say, Fastmail's JMAP) means writing one new crate that implements it, with no changes to the engine. This is the "keep providers separate" rule from `CLAUDE.md`, enforced by the compiler.

## The ideas it defines

**Objects.** Every service stores events as *objects*: one item at an address (`href`) with a version (`etag`) and raw data (`data`). For CalDAV that's an `.ics` file; for Google and Microsoft it's JSON. A recurring series and its exceptions form **one** object.

**`ObjectCodec`** converts a provider's raw data to and from `calendar-core`'s `ObjectContent`. It's pure and synchronous (no network), so the engine can run it on a blocking thread.

```rust
pub trait ObjectCodec: Send + Sync {
    fn read(&self, data: &str) -> Result<ReadObject, String>;
    fn write(&self, base: Option<&str>, uid: &str, content: &ObjectContent,
             renamed: &[(OccurrenceKey, OccurrenceKey)]) -> String;
}
```

`write` receives `base`, the last server copy, so the codec can **edit it** instead of starting over. That keeps unknown fields intact.

**`CalendarProvider`** is the network side:

```rust
#[async_trait]
pub trait CalendarProvider: Send + Sync {
    fn codec(&self) -> Arc<dyn ObjectCodec>;
    async fn list_calendars(&self) -> ProviderResult<Vec<RemoteCalendar>>;
    async fn fetch_changes(&self, calendar: &RemoteCalendar, cursor: Option<&str>,
                           known: &[(String, Option<String>)]) -> ProviderResult<ChangeSet>;
    async fn put_object(&self, href: &str, payload: &str, precondition: &Precondition)
                           -> ProviderResult<WriteResult>;
    async fn get_object(&self, href: &str) -> ProviderResult<Option<RemoteObject>>;
    async fn delete_object(&self, href: &str, version: Option<&str>) -> ProviderResult<()>;
    fn take_updated_secret(&self) -> Option<String> { None }   // default method
}
```

**`Precondition`** makes writes safe. A write is `Create` (it must not exist yet), `Version(etag)` (only if nobody changed it since we last read it), or `Overwrite` (the user chose "keep mine"). If someone else changed the event, the server refuses, and the engine sees `PreconditionFailed` instead of silently overwriting their change.

**`ProviderError`** sorts every failure into a kind the engine can act on:

| Error | Engine's reaction |
|---|---|
| `Network`, `Server`, `RateLimited` | temporary (`is_transient()`): retry later with backoff |
| `Unauthorized` | ask the user to sign in again |
| `PreconditionFailed` | try a three-way merge, otherwise mark a conflict |
| `SyncTokenInvalid` | start a full re-sync of that calendar |
| `NotFound`, `Forbidden`, `InvalidResponse` | report to the user |

**`Capabilities`** says what a calendar supports (incremental sync? conditional writes?), because services differ.

## Rust concepts in this file

- **Traits as contracts.** The whole point of the crate ([concepts §6](concepts.md#6-traits-shared-behaviour)).
- **`Send + Sync` supertraits.** `trait CalendarProvider: Send + Sync` means every provider must be safe to use from several threads, which the async engine needs ([§13](concepts.md#13-smart-pointers-and-sharing-box-arc-mutex)).
- **Trait objects:** `Arc<dyn ObjectCodec>` and, in the engine, `Box<dyn CalendarProvider>` ([§7](concepts.md#7-generics-and-trait-objects-dyn)).
- **`#[async_trait]`.** Lets trait methods be `async` while still usable through `dyn` ([§14](concepts.md#14-asyncawait-and-tokio)).
- **Default methods.** `take_updated_secret` has a body, so only OAuth providers need to override it.
- **Enum variants with named fields:** `RateLimited { retry_after: Option<Duration> }`.
- **`matches!`** for a neat `is_transient()`.
- **`#[derive(Default)]`** on `ChangeSet`, so providers can start with `ChangeSet::default()` and fill it in.
