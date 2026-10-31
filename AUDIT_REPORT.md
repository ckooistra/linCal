# Independent technical audit — Universal Calendar

**Date:** 2026-10-29  
**Revision:** `b543d7268442a56df00b0e1e8ee01c4d24615f52`  
**Scope:** Read-only review of the implemented desktop application, engine, persistence, CalDAV integration, tests, tooling, and documentation. This report is the only repository addition made for the audit. No fixes were implemented.

## 1. Assessment

**The architectural foundation is sound, but Milestone 3 should not be considered validated for reliable use with important calendars.** Ordinary CRUD and synchronization work, including the repository's two-client tests against an isolated Radicale server. Adversarial checks nevertheless reproduced loss of deletion intent, destructive recurrence round trips, unrecoverable conflict states, malformed-input panics, and incorrect time changes during editing.

Responsiveness is partly demonstrated, rather than universally achieved. SQLite queries are fast on the supplied datasets. Several GUI targets fail in the measured Xvfb/software-rendering environment, including month navigation at 1,000 events and dense week rendering at 5,000. Actual pointer-driven 60 FPS dragging, KDE/GPU behavior, and responsiveness during large account synchronization remain unverified.

There are **9 High, 14 Medium, and 1 Low findings** below. High means a demonstrated or strongly established risk to data correctness, credential confidentiality, or continued operation. Medium means a material reliability, responsiveness, extensibility, or usability defect. Low means documentation/process drift. These are engineering priorities, not CVSS ratings.

### Milestone and requirements

- No `AGENTS.md` exists in the repository or the inspected ancestor directories. `CLAUDE.md` identifies itself as “AGENTS.md” internally, but is a different filename.
- Read before implementation review: `PROJECT_SPEC.md`, `README.md`, all architecture/performance/provider/synchronization documents, all four ADRs, the dependency review, and `CLAUDE.md`.
- README, architecture docs, ADR 0004, and the actual implementation establish **Milestone 3: CalDAV**. Git history also records the Milestone 2 and 3 implementation commits.
- `PROJECT_SPEC.md` and `CLAUDE.md` still direct development toward Milestone 1. This is a genuine documentation conflict; repository history alone does not establish whether later work was separately authorized. The audit assesses the features actually claimed for Milestone 3 without assuming Milestones 4 or 5 should exist.
- Missing Google OAuth/API, Microsoft OAuth/Graph, ICS subscriptions, agenda, notifications, invitations, and “this and following” editing are **planned scope**, not findings merely because they are absent.

## 2. Inventory and architectural conformance

| Area | Actual implementation | Assessment |
|---|---|---|
| `calendar-core` | Provider-neutral models, validation, zoned/floating/all-day timing, occurrence keys, `rrule` expansion, wire types; optional simulated data | No Tauri or I/O dependency; appropriate separation |
| `calendar-storage` | SQLx SQLite pool, two embedded migrations, CRUD, series/exception operations, preferences, remote objects, durable operation queue | Real persistence, not a mock; important consistency defects below |
| `calendar-ical` | Custom content-line parser/writer and VEVENT mapping | Preserves much raw semantic content, but the lossless claim exceeds demonstrated behavior |
| `calendar-provider-api` | Async provider trait, object/version/change types, classified errors, two capability flags | Useful CalDAV seam; restrictive for future JSON providers |
| `calendar-caldav` | Basic authentication, discovery, listing, sync-collection, ETag fallback, multiget batches of 50, conditional PUT/DELETE | Real HTTP implementation, with mock and optional real-server tests |
| `calendar-sync` | Credential abstraction, keyring integration, injected provider factory, push/list/pull passes, retry/conflict handling, scheduler | Tauri independent; global serialization and lifecycle weaknesses |
| Tauri application crate | Async commands, startup SQLite initialization, sync event forwarding, window/benchmark hooks | Mostly thin integration; a create command misses a necessary scheduling action |
| Svelte UI | `App`, sidebar/toolbar/title bar, editor, account/removal dialogs, time picker, toasts, performance panel | Svelte 5 runes, strict TypeScript, logical component boundaries |
| State and services | EventStore with optimistic changes/range cache, calendar/status store, notification store, bridge/backend/EventSource | Provider-neutral UI; cache and asynchronous ordering problems remain |
| SVAR | Exact pin `@svar-ui/svelte-calendar` 2.7.1; isolated adapter and mapping module | Installed package and license text are MIT; no application import of PRO functionality found |
| Preview/demo | Browser-only MockEventSource, TypeScript generator; desktop Rust generator and SQLite demo accounts | Explicit simulations, correctly distinguished from real provider support |
| Tests/tooling | 10 frontend test files, Rust unit/integration tests, deterministic datasets, SQLite and real-WebKit GUI benchmarks, screenshot and Radicale scripts | Useful coverage; major failure-path and lifecycle gaps |
| CI/CD | No checked-in CI workflow or release pipeline found | Manual checks are not enforced on future changes |
| Packaging | Tauri `.deb`/AppImage configuration and icons | Release executable built; package installation/distribution not audited |

Dependency direction is acyclic and sensible: sync depends on storage, iCalendar, and CalDAV; iCalendar/storage depend on core; CalDAV depends on provider-api. `cargo tree -p calendar-sync --edges normal` contains no Tauri dependency. SQLite supplies displayed desktop data; network synchronization runs separately. This is a real local-first implementation.

Positive controls include parameterized SQL, enabled foreign keys, WAL, transactions for event-plus-queue writes, client-generated IDs, conditional remote updates, per-object queue generations, range-limited IPC queries, recurrence work on `spawn_blocking` for reads, system-keyring password storage, HTTP timeouts, same-origin HTTP redirect checking, restricted CSP, and no required hosted backend. These controls deserve preservation.

No fake Google/Microsoft adapter or placeholder success implementation was found. The explicit mocks, simulated latency/failures, pre-expanded browser recurrence, and performance drag proxy are development facilities. Some unused-looking provider variants and capabilities are forward declarations, not implemented providers. The TypeScript/Rust generators and Rust/TypeScript wire shapes duplicate functionality intentionally; their long-term drift risk is real, but they are currently used. No dependency cycle or demonstrably unnecessary major runtime dependency was established.

## 3. Findings

Evidence labels: **Reproduced** means executed using temporary probe code and isolated data; **Static** means established from the code path; **Measured** means observed by the supplied benchmark. Recommendations describe follow-up work only.

### F01 — High — Deleting an event during its first upload loses deletion intent

**Evidence: Reproduced.** `crates/calendar-storage/src/remote.rs:236` treats a NULL ETag as “never reached the server” and deletes both object metadata and queue entries at lines 258–269. A PUT can already be in flight with a snapshot of that object. `complete_put` then updates a nonexistent metadata row, and no DELETE remains queued.

**Reproduction:** create a remote-calendar event; obtain its due operation and push snapshot; delete it locally; complete the previously captured PUT successfully. The probe found zero pending operations and no object metadata. On a real successful PUT the server event remains and may reappear on the next pull. The operation generation counter does not protect the branch that forgets the object entirely.

**Required follow-up:** distinguish unsent creation from potentially submitted creation; retain durable deletion intent across in-flight requests, ambiguous network outcomes, and restart. Exercise this with a delayed provider, not only sequential storage calls.

### F02 — High — “Use server version” / discard can leave falsely synced ghost events

**Evidence: Reproduced.** `remote.rs:947` deletes the queue entry, clears the ETag, and marks event rows `synced`. For a never-uploaded object it deletes metadata but leaves event rows. For an event deleted on the server, `known_objects` at line 512 excludes the now-NULL ETag, so a full listing cannot infer that this known href disappeared.

**Reproduction:** (a) fail a new create permanently, then discard it; (b) import an event, edit it locally, conflict with a remote deletion, then choose the server version. Both probes retained a local event marked `synced` with no queued operation. In case (a), push metadata was gone; in case (b), the href was absent from deletion-comparison input.

**Required follow-up:** reconcile actual server existence before completing discard resolution, and represent “known resource, version needs refresh” separately from “new resource.”

### F03 — High — Shifting a series drops unmodeled data from its overrides

**Evidence: Reproduced.** `calendar-storage/src/lib.rs:385` changes override recurrence keys. `calendar-ical/src/events.rs:484` matches original VEVENT components only by their old recurrence key; at lines 503–508, unmatched overrides become fresh components containing a UID and modeled fields.

**Reproduction:** import a recurring object with an override containing `ATTENDEE` and an `X-` property; shift the entire series by one hour; serialize the push object. Both properties disappeared from the override despite being present in the stored base copy. The same mechanism threatens override alarms and other unmodeled fields.

**Required follow-up:** retain a durable association with the original component when recurrence identity changes. Round-trip tests must cover structural edits, not only text changes to unchanged component identities.

### F04 — High — RDATE-only recurrence is silently lost on write

**Evidence: Reproduced.** `calendar-ical/src/events.rs:227` creates a recurrence model only when RRULE exists. A valid recurrence set specified with DTSTART/RDATE and no RRULE is treated as a one-off. The writer's `None` recurrence branch removes RDATE and EXDATE.

**Reproduction:** parse an event with `RDATE:20260611T090000Z` and no RRULE, then pass its unchanged modeled content to `write_events`. The model had no recurrence and the output contained no RDATE. Extra dates are missing in display and can be removed remotely by an otherwise ordinary edit.

**Required follow-up:** model recurrence sets independently of the existence of RRULE; preserve unsupported recurrence instead of silently removing it. RFC 5545 defines recurrence from the union of applicable starts, rules, and explicit dates. [RFC 5545 §3.8.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.5)

### F05 — High — Malformed remote updates remove the last valid local event

**Evidence: Reproduced.** `remote.rs:621` deletes the existing event rows and overwrites the remote object/ETag before the per-object savepoint is created at line 645. Parse failure or insertion failure therefore cannot restore the previous rows. The new version is recorded as known and the cursor can advance.

**Reproduction:** import a valid event at ETag v1, then apply malformed content for the same href at v2. Visible event count changed from one to zero, v2 became the known version, and the failure was recorded. Raw malformed content remains, but the last good local representation does not. This is not a silent overwrite of pending local edits—the pending guard does protect those—but it is destructive failure recovery.

**Required follow-up:** preserve the last good object until replacement succeeds; quarantine failed versions separately, and expose actionable import failures to the UI.

### F06 — High — Untrusted calendar data can panic and terminate the background scheduler

**Evidence: Reproduced panic; Static lifecycle consequence.** `calendar-ical/src/values.rs:69` uses unchecked integer arithmetic. `events.rs:113` constructs Chrono durations and adds dates without checked bounds. A remote `DURATION:P999999999999D` panicked in `read_events` with `TimeDelta::days out of bounds`. `P9223372036854775807W` also overflowed in the debug parser probe.

`calendar-sync/src/engine.rs:339` parses remote objects directly in the scheduler task, outside an isolation boundary. `apps/desktop/src-tauri/src/lib.rs` spawns that scheduler and discards its task handle. A panic unwinds the task; there is no restart/supervision path, so subsequent scheduler triggers cannot restore synchronization until application restart. This need not crash the entire GUI to be a serious availability failure.

**Required follow-up:** checked parsing/arithmetic, bounded input, per-object fault containment, supervised scheduler lifecycle, and malformed/oversized-input tests. Moving parsing to a worker alone would not fix unchecked operations.

### F07 — High — Credential transport restrictions are weaker than documented

**Evidence: Static.** `calendar-sync/src/engine.rs:184` accepts arbitrary `http://` origins. `calendar-caldav/src/client.rs:85` sends HTTP Basic credentials. This allows plaintext authentication to a non-loopback server despite the documented TLS requirement.

Same-origin checking protects HTTP redirects within one client, but discovery returns an unchecked `calendar-home-set` URL (`calendar-caldav/src/lib.rs:114`). Account setup then constructs a new client bound to that returned origin (`engine.rs:195`), carrying the original password. A foreign or downgraded home URL can therefore bypass the intended credential-origin boundary without using an HTTP redirect. Cross-origin discovery can be legitimate for some providers, but needs an explicit trust policy.

Also, server URLs are stored verbatim in account config; userinfo is not rejected. An embedded password supplied in a URL could be persisted as “non-secret” configuration. No real credentials were used to test these paths.

**Required follow-up:** enforce HTTPS outside narrowly scoped test/loopback allowances, validate discovery trust transitions, and reject/redact credential-bearing URLs before persistence or logging.

### F08 — High — Old/high-frequency recurring events silently disappear

**Evidence: Reproduced.** `calendar-core/src/recurrence.rs:165` stops after 50,000 candidates counted from DTSTART, returning ordinary success. It does not seek directly to the requested range. `has_occurrence` has the same limitation.

**Reproduction:** import an unbounded hourly series starting 2010-01-01 and query June 2026. Its master remains stored, but the result is empty. A series only several years old can exceed this cap at hourly frequency. The limit is useful for resource protection, but a successful incomplete answer is not safe calendar behavior.

**Required follow-up:** range-aware expansion/caching and explicit completeness errors when limits are reached; validate editing/deleting occurrences beyond the current cap.

### F09 — High — A title-only save can change an event's instant during a DST overlap

**Evidence: Reproduced conversion path.** `EventEditor.svelte:59` reduces existing instants to local dates plus HH:MM. `buildTiming` at line 139 recreates them using `services/time.ts:43`, which selects the earlier ambiguous local time and zeros seconds/milliseconds.

**Reproduction:** with `TZ=Europe/Berlin`, round-trip `2026-10-25T01:30:45Z` through the editor's `toIsoDate` / `toTimeString` / `combineDateTime` calls. It became `2026-10-25T00:30:00Z`, a change of one hour and 45 seconds. Even outside a DST overlap, nonzero seconds are discarded on an unchanged-time save.

**Required follow-up:** preserve the exact original timing unless a timing field changes; explicitly handle ambiguous times when users actually edit them. Test title-only edits on both sides of the DST fold.

### F10 — Medium — Some conflict/failed-operation states cannot be resolved through the UI

**Evidence: Reproduced storage states; Static UI consequence.** Deletions remove event rows before upload. If DELETE gets a permanent failure or conflict, `object_of(deleted_event_id)` returns no result, while the only conflict command/UI requires an existing event/editor. The sidebar reports a count but provides no operation-level resolution route.

Separately, `remote.rs:217` preserves queue state `conflict` on a subsequent edit, but line 226 changes the event status to `pending-sync`. The probe confirmed a conflict still counted in account status, no due operation, and an event no longer marked conflict. The editor's resolution banner is driven by that event status and disappears.

**Required follow-up:** make failed operations independently inspectable/resolvable, including tombstones; maintain consistent event/queue status after further edits. Preserve conditional retry semantics rather than treating every “Try again” as unconditional overwrite.

### F11 — Medium — Missing ETags conflate existence, version uncertainty, and local creation

**Evidence: Static.** `complete_put` accepts a missing response ETag and overwrites the existing one (`remote.rs:819`). The engine later maps no ETag to create (`engine.rs:404`), while deletion logic maps it to “never uploaded.” Another local edit during the upload, followed by a response without ETag, can leave a newer pending update using `If-None-Match:*` against an existing resource. Deletion in this window can hit F01's forget branch. `WriteResult.href` is also ignored by completion logic.

A CalDAV server may legitimately omit a strong ETag after transforming submitted data; the client must refresh the authoritative representation/version. [RFC 4791 §5.3.4](https://www.rfc-editor.org/rfc/rfc4791.html#section-5.3.4)

**Required follow-up:** represent remote existence separately from the last known version, refresh after uncertain write responses, reconcile returned identity, and cover ambiguous outcomes/crash replay with provider-level tests. Duplicate submissions are protected from blind overwrite by preconditions, but are not fully recovered automatically.

### F12 — Medium — Creating an event does not wake synchronization

**Evidence: Static.** `apps/desktop/src-tauri/src/commands.rs:162` persists creation and returns without `state.changed(event.sync_status)`. Update and delete commands do trigger the scheduler. After an idle pass has gone to sleep, a new event can wait for the five-minute interval or another action/Sync now.

**Required follow-up:** trigger the existing background path after durable creation; test command-to-scheduler behavior. This contradicts the documented “a few seconds after local edits” behavior.

### F13 — Medium — An older range response overwrites a completed optimistic edit

**Evidence: Reproduced.** `events.svelte.ts:276` protects only changes currently marked `pending-local`. It does not compare request age or a mutation generation. A query can capture an old snapshot, the edit can persist successfully, and then the old response can replace both the current cache and confirmed rollback state.

**Reproduction:** begin a controlled range query returning the original event; complete a local rename; release the original query result. The cache reverted to the old title. SQLite remains updated in this scenario, but the UI is wrong and future rollback now uses stale data. Deletion tombstones are also removed on completion before all older loads have necessarily returned.

**Required follow-up:** order loads against mutations using store/request epochs or equivalent versioning; serialize/coalesce per-event writes and guard mutation completions across source resets. The latter completion-order/reset cases are static risks, not independently measured data-loss claims.

### F14 — Medium — Sync status is overwritten or not refreshed in visible events

**Evidence: Reproduced status overwrite; Static notification gaps.** `events.svelte.ts:271` always replaces the backend's status with `saved`, including a returned `pending-sync` status. Series edit completion also sets `saved` directly.

Permanent push failure does not contribute to `SyncReport.changed()` (`engine.rs:70`); transient failure emits `failed`. `App.svelte:135` reloads event data only for successful passes with `changed=true`, and the failed branch updates account summaries only. Thus an event can remain visually saved/pending after SQLite marks it failed. Import failures likewise need not invalidate a removed event's cached rendering.

**Required follow-up:** preserve returned lifecycle status, propagate affected-object/status changes independently of successful download counts, and update an already-open editor when its backing event changes.

### F15 — Medium — Authentication/keyring failures can cause one-second retry loops; accounts are globally serialized

**Evidence: Reproduced scheduling input; Static loop behavior.** A pending operation starts with `next_attempt_ms=0`. Missing credentials and unauthorized failures do not reschedule it. The probe confirmed `next_retry_ms()==Some(0)` after missing-password failure. `scheduler.rs:47` turns an overdue timestamp into a one-second wait and reruns all accounts. A fast failure can repeat roughly once per second, including keyring calls and status writes.

The single engine mutex (`engine.rs:105`) covers whole account passes, and `sync_all` runs accounts sequentially. Slow responses or many queued writes in the first account delay every other account and account removal. One request has a 60-second timeout, but there is no bounded whole-pass duration. Returned errors are isolated; latency and panic failure are not adequately isolated.

**Required follow-up:** suspend/retry authentication failures deliberately, add account-level scheduling/backoff, and use bounded per-account concurrency with task cancellation/shutdown handling. The 1.5-second delay is coalescing, not a true quiet-period debounce.

### F16 — Medium — Unsupported incoming calendar semantics are silently approximated

**Evidence: Reproduced cancelled-series display and early-RDATE omission; otherwise Static.**

- `events.rs:240` filters a cancelled master only when it has no recurrence. A cancelled daily master with COUNT=2 was displayed twice by the probe.
- `codec.rs:175` indexes the start from DTSTART, not the earliest RDATE. A May 1 RDATE on a series with June 10 DTSTART was omitted from a May 1 query, even though it is a member of the recurrence set.
- `RECURRENCE-ID;RANGE=THISANDFUTURE` is read as one override; the range parameter has no modeled meaning. Not offering future-series editing is planned scope; incorrectly interpreting an incoming range exception is a current import problem.
- `values.rs:32` reduces PERIOD-valued RDATE to its start, losing occurrence-specific duration in display/modeling.
- Unknown TZIDs become floating; VTIMEZONE transition definitions are not evaluated. Warnings are discarded by `ObjectEvents::into_content`, so users cannot distinguish a reliable instant from an approximation. Moving such an event can replace its original TZID with floating timing.

**Required follow-up:** accurately support these incoming cases or preserve them with an explicit unsupported/read-only state. Do not advertise universal DST-correct/lossless behavior for approximated objects. These meanings are specified by [RFC 5545](https://www.rfc-editor.org/rfc/rfc5545.html).

### F17 — Medium — Whole-series edits mis-handle DST exception shifts and changed rule membership

**Evidence: Reproduced.** `recurrence.rs:242` computes a fixed UTC delta, and `calendar-storage/src/lib.rs:595` adds that delta to all RDATE/EXDATE keys. That does not preserve exception wall time when a date move crosses a DST boundary for an exception but not for DTSTART.

**Reproduction:** a Berlin daily series starting March 25, 2026, COUNT=10, with March 29 09:00 excluded has nine instances. Shift the series one day earlier through its first occurrence. The EXDATE shifts to March 28 08:00 local while the series occurs at 09:00; the probe returned ten instances. Separately, shrinking COUNT from five to two retained a pre-existing day-five override and returned three occurrences.

**Required follow-up:** define calendar/time-zone-aware series-edit semantics, remap exceptions accordingly, and reconcile overrides against the new rule. Preserve raw override fields while doing so (F03).

### F18 — Medium — Derived IDs are lossy and can collide for distinct remote resources

**Evidence: Reproduced object collision; Static additional identity risks.** `remote.rs:124` uses only the final href segment, strips `.ics`, and replaces `::` with `:_:`. Distinct hrefs ending `/x` and `/x.ics` map to the same event ID. The probe imported only one event, recorded an insertion failure for the other, and acknowledged both raw objects. Calendar IDs at line 321 also use only a final path segment, which can collide across different collection paths within an account.

New href sanitization is similarly many-to-one; `@` is used heuristically to recognize lone recurrence instances, so filenames/client IDs containing it can change classification/identity. The LIKE expression at line 582 also embeds resource-derived `%`/`_` in a pattern without escaping; this deserves dedicated cross-object association tests.

**Required follow-up:** independent stable internal IDs with unique account/calendar/provider-resource mappings; avoid deriving semantic type from punctuation in an ID. UID alone must not replace resource-scoped identity.

### F19 — Medium — Measured GUI responsiveness misses several explicit targets

**Evidence: Measured.** See section 4 for the current release build. At 1,000 events month-next is 60/107 ms p50/p95 against <50 ms, visibility toggle is 31/42 ms against <30 ms, and the drag proxy achieves 52 FPS. At 5,000, week navigation is about 104–109 ms p50 and the proxy achieves 12.9 FPS.

The proxy is not an actual drag benchmark, and software rendering limits applicability. These numbers establish misses in this environment, not the exact bottleneck on KDE/GPU hardware. Do not treat M1/M2 documentation as proof that all current targets are met.

**Required follow-up:** measure actual pointer interactions and first-complete-event paint on target KDE hardware; profile dense-week rendering and repeated reset/reload paths before choosing optimizations.

### F20 — Medium — Cache/work growth is not bounded across navigation or large syncs

**Evidence: Reproduced cache retention; Static workload risks.** EventStore retains events and confirmed copies for every visited range without eviction. Loading 120 disjoint test ranges retained all 120 event objects. `eventsIn` and merge cleanup then scan the accumulated cache. Range queries are individually bounded, but frontend memory is not bounded over a session.

`TauriEventSource` drops obsolete results but cannot cancel backend work. Every `query_range` may spawn blocking recurrence work without an application semaphore; rapid navigation can leave multiple obsolete queries consuming resources. There is no recurrence expansion cache. SVAR receives a complete prefetched window reset after loads, though individual edits are incremental.

The SQLite query fetches all calendars, including hidden ones. Large pulls collect all changed raw objects, parse them on an async runtime worker, and apply a whole calendar in one write transaction; finite-series index computation also runs inline during writes. These can delay local persistence and other tasks even though they do not directly run on the webview thread. HTTP response bytes, total object count, and parser nesting are not explicitly capped. Request/multiget batching does not bound total pass memory.

**Required follow-up:** bounded range-cache retention, cancellable/bounded CPU jobs, visible-calendar-aware query policy where appropriate, measured sync batches, and bounded input/transaction work. Large real-account starvation/RSS limits were not benchmarked.

### F21 — Medium — The provider contract makes future Google/Graph support unnecessarily costly

**Evidence: Static architecture risk; not a demand to implement Milestone 4 now.** `calendar-provider-api/src/lib.rs` requires href-addressed iCalendar objects and PUT semantics; storage generates remote `.ics` URLs itself. Provider-specific native JSON has no explicit preservation envelope. Forcing every provider through iCalendar needs a reversible native-data mapping to avoid losing fields outside that format. Google separates its event ID and iCalUID and exposes provider-specific data. [Google event resource](https://developers.google.com/workspace/calendar/api/v3/reference/events)

The contract and schema have one cursor per calendar, with no typed range/window identity or persisted page state. Microsoft calendar-view delta tracks a particular calendar and date range and follows opaque next/delta links; mapping that onto one unqualified cursor is not sufficient for multiple windows. [Microsoft calendar-view delta](https://learn.microsoft.com/en-us/graph/delta-query-events)

Capabilities only describe incremental sync and conditional writes; no per-operation create/update/delete/recurrence/attendee support reaches the UI. Factory authentication accepts one password string; OAuth refresh/lifecycle and provider-assigned create identity have no explicit contract. These are changeable seams, but the documented claim that JSON mapping alone leaves the engine unchanged is premature.

**Required follow-up:** settle native metadata preservation, identity/create results, capability propagation, authentication lifecycle, and scoped cursors before implementing another provider. Keep the existing Tauri-free boundaries.

### F22 — Medium — Advertised Rust minimum is incompatible with locked dependencies

**Evidence: Installed dependency manifests.** Workspace `rust-version` and README advertise Rust ≥1.90. Locked SQLx 0.9.0 and sqlx-core 0.9.0 declare `rust-version = "1.94.0"`. Builds passed on Rust 1.99.1; they do not validate the advertised minimum. No alternate toolchain was installed for this audit.

**Required follow-up:** align the stated minimum and dependency policy, and check the supported toolchain in CI. At least Rust 1.94 is required by these dependencies; this is not a claim that every target's complete graph was tested on 1.94.

### F23 — Medium — Keyboard/modal access is incomplete

**Evidence: Static.** EventEditor has a focus trap and focus restoration, but AddAccountDialog and ConfirmDialog only set initial focus and handle Escape; background controls are not made inert. Tab can escape dialogs marked modal. Busy account discovery disables cancellation, and there is no cancellable backend discovery operation.

At widths below 720 px `App.svelte` hides the entire sidebar, with no alternate calendar/account/settings control. The window minimum is 640 px. Core calendar event markup/interaction is not covered by the component tests, which do not exercise the real SVAR adapter. Screen-reader and keyboard event-grid behavior therefore remain unverified despite passing Svelte diagnostics.

**Required follow-up:** consistent modal focus management, usable compact-width account/calendar controls, cancellable long setup, and actual widget keyboard/screen-reader testing.

### F24 — Low — Documentation mixes milestones and overstates validated guarantees

**Evidence: Static.** `CLAUDE.md`/`PROJECT_SPEC.md` still prioritize M1; README claims M3 and broad lossless/DST-correct behavior contradicted above. Performance docs mix M1 pre-expanded datasets with M2 recurrence data and retain a “Milestone 2 next step” already implemented. Architecture text still describes four tables before later noting migration 2, and its capability description omits newly added window permissions. Sidebar dataset text says “replaces all events,” though storage correctly replaces simulated accounts only.

**Required follow-up:** establish one active-milestone authority; qualify compatibility/losslessness claims, align UI descriptions, and record current M3 evidence and remaining acceptance gaps. The lack of a live Nextcloud run is explicitly acknowledged in sync docs and should remain visible.

## 4. Performance investigation and measurements

### Method and limits

Rebuilt the frontend production bundle and release executable from the audited source. The desktop build used `--features tauri/custom-protocol` so the executable loads its embedded frontend. An initial direct-Cargo build used the development URL and produced no benchmark data; that audit-only run was stopped and excluded.

Ran the repository's `scripts/bench.py` using a **new temporary `UC_DATA_DIR`** for both its benchmark and idle phases, `UC_CREDENTIALS=memory`, and an explicit output path outside the repository. This isolation is necessary: the script's idle phase otherwise launches normally and can open the ordinary user database. No production database was opened by the audit benchmark.

Environment: Ubuntu 24.04.1, kernel 6.8.0-110-generic, QEMU CPU, 8 logical CPUs, 7.8 GiB RAM, WebKitGTK 2.52.6, 1280×820 viewport, Xvfb/software rendering. One run per dataset suite; small samples and shared VM load limit statistical confidence. Benign portal/PipeWire warnings occurred, but the benchmark completed successfully.

The suite includes eight enabled simulated calendars, overlapping events, month/week/day changes, rapid navigation, repeated visibility changes and edits, search, and a completed-move-per-frame proxy. Results measure action to next rendering opportunity using rAF/MessageChannel; they do not prove that every async data reload has completed before each recorded frame. At cold/obsolete ranges a fast navigation frame can contain incomplete event data. The drag proxy persists each step; actual pointer dragging/resizing is a separate, still-unverified target.

### Current GUI results

Numbers are **p50 / p95 milliseconds**, except FPS. Search measures applying a filter over loaded events; the application's **150 ms debounce is excluded**. It is neither all-history SQLite search nor end-to-end keystroke latency.

| Interaction | 100 events | 1,000 events | 5,000 events | Target/interpretation |
|---|---:|---:|---:|---|
| Month next | 37 / 70 | 60 / 107 | 36 / 124 | <50 ms; tails miss in all scenarios |
| Month previous | 31 / 50 | 47 / 66 | 43 / 122 | <50 ms; tails miss |
| Switch to week | 23 / 80 | 49 / 61 | 147 / 179 | Dense week noticeably expensive |
| Week next | 17 / 23 | 37 / 69 | 104 / 169 | 5,000-event latency substantial |
| Visibility toggle | 14 / 22 | 31 / 42 | 48 / 61 | <30 ms; 1,000 and 5,000 miss |
| Open editor | 31 / 162 | 29 / 33 | 48 / 54 | <50 ms; 100-event outlier and 5,000 tail miss |
| Create, optimistic paint | 10 / 13 | 12 / 26 | 16 / 20 | Immediate visual feedback demonstrated |
| Search filter application | 15 / 30 | 16 / 48 | 17 / 66 | <100 ms for this limited scope |
| Visible query IPC round trip | 1.7 / 2.0 | 1.8 / 1.9 | 3.1 / 3.6 | <100 ms for queried day ranges |
| Completed-drag proxy | 63.4 FPS | 52.0 FPS | 12.9 FPS | Does not certify actual drag FPS |
| Ten rapid month steps, one burst | 37 | 55 | 79 | Single samples, not settled-load percentiles |

At 5,000 events, week-next script/layout p50 was 29/44 ms, versus 104 ms total; week-previous was 32/46 ms, versus 109 ms total. The remaining delay is consistent with significant rendering/compositing cost, but this timing split does not establish that application code is irrelevant. Extra async store resets and forced layout also participate. The claim “application code isn't the bottleneck” is stronger than this evidence supports.

Idle sample: **0.13% of one CPU core** over 30 seconds after eight seconds settling; approximately **374 MiB aggregate RSS** over three app/WebKit processes. Scenario-end aggregate RSS: **452 / 487 / 535 MiB** for 100 / 1,000 / 5,000. RSS includes WebKit/shared mappings and is not a unique-memory allocation measurement. No failing account was configured during idle measurement; F15's failure-mode CPU/network behavior is outside this idle result.

### SQLite + recurrence benchmark

Executed the existing release `bench_queries` example. It creates and removes its own temporary on-disk databases. Query timings include decode and recurrence expansion, with 12 ranges per scenario.

| Rows in generated dataset | Dataset transaction | Week p50 / p95 | Six-week month p50 / p95 | Three-month prefetch p50 / p95 |
|---|---:|---:|---:|---:|
| 100 | 8.50 ms | 0.74 / 0.88 ms | 0.88 / 1.23 ms | 1.62 / 1.89 ms |
| 1,000 | 69.09 ms | 1.43 / 1.77 ms | 3.20 / 5.78 ms | 6.10 / 10.24 ms |
| 5,000 | 327.34 ms | 3.60 / 4.22 ms | 10.32 / 17.91 ms | 20.30 / 29.91 ms |

These results support fast local retrieval for these synthetic datasets. They do not establish query behavior with thousands of old recurring masters, huge descriptions, long exception lists, concurrent initial sync, or a lifetime-sized database. The SQL date index can prune by start, but the interval condition and unbounded series still require examining many candidates; the calendar/date index is not used to filter enabled calendar IDs by the current query.

## 5. SQLite, standards, security, and maintainability conclusions

### Persistence and consistency

The schema supports multiple accounts per provider, multiple calendars per account, recurring masters, overrides, raw object metadata, per-calendar sync tokens, and persistent coalesced operations. Foreign-key cascades are appropriate for intentional account/series deletion. Related local writes are transactional, and basic reopen/queue persistence tests pass. Migrations are embedded SQLx migrations; no destructive migration was run against existing user data.

Important limitations beyond the findings:

- `revision` is not an enforced expected-version precondition for local mutation commands; concurrent frontend writes are not ordered by a durable client mutation sequence.
- Range candidates and override keys are loaded in separate reads, without one read snapshot. A concurrent override mutation can produce a transient inconsistent query result. This race was not independently reproduced.
- WAL `synchronous=NORMAL` is a performance/durability choice: process-crash recovery should be distinguished from guarantees during machine power loss. No power-cut test was performed.
- There is no bounded occurrence table, which avoids a major storage-growth trap. There is also no recurrence expansion cache yet.
- Attendees, organizer, alarms, visibility/availability and unmodeled fields are mostly retained inside raw iCalendar rather than first-class core models. That is a reasonable early preservation strategy only if structural edits remain lossless; F03/F04 show they currently do not.
- There is no native provider-metadata store/scoped cursor model suitable for all later providers yet. This is architectural preparation work, not evidence that planned integrations already exist.

### Calendar correctness matrix

| Case | Audit conclusion |
|---|---|
| UTC, common IANA zones | Implemented, with ordinary unit coverage |
| Floating times | Explicit model; resolved in viewer zone; ambiguous-time conversion remains problematic |
| All-day events | Date representation with exclusive end, useful timezone tests |
| RRULE daily/weekly/monthly/yearly | Implemented via established `rrule` crate, not fixed-second recurrence generation |
| DST recurrence | Ordinary wall-clock expansion tested; series exception retiming/editor round trips fail specific cases |
| RDATE/EXDATE | Supported with RRULE in ordinary cases; standalone RDATE and early-RDATE bounds fail |
| RECURRENCE-ID | Stable occurrence/override identities in common paths; structural preservation and rule reconciliation fail |
| RANGE / PERIOD / custom VTIMEZONE | Incomplete semantics, insufficient user-visible limitation signaling |
| Cancellation | EXDATE and cancelled overrides partly supported; recurring master cancellation fails |
| Read-only calendars | Both UI/store and storage enforce broad read-only status; fine-grained bind/unbind/update rights are not modeled |
| Duplicate identity | Resource-based approach is sensible, but lossy ID mapping collides |
| Organizer/attendee/reminder editing | Not implemented; raw preservation is partial and fails under F03 |
| “This and following” editing | Explicit later scope; absence is not a current milestone failure |

### Security and dependencies

No embedded OAuth client secret, plaintext dedicated password column, shell execution plugin, broad filesystem/network frontend permission, telemetry service, or required proprietary backend was found. Password calls use `spawn_blocking` for keyring access. Real KWallet/GNOME Keyring storage and cross-platform credentials were not exercised; tests used in-memory dummy credentials. TLS certificate verification is enabled by the chosen reqwest configuration; it was not tested against enterprise roots here.

The CSP permits inline styles for widget styling but restricts scripts and network origins. Generated calendar CSS validates hex colors and sanitizes identifiers. This is a useful injection control, although CSS identifier normalization can collide. Tauri custom application commands still provide substantial data/network authority; absence of a generic HTTP permission does not sandbox those Rust commands. No direct script-injection exploit was established.

`npm audit --json` during the audit returned **two low findings**, for `deepmerge` and its affected direct build-tool dependency `@sveltejs/vite-plugin-svelte`; no fix was reported. This agrees with the dependency document and is not a reason to upgrade automatically. `cargo-audit` is not installed, and no RustSec advisory scan was claimed or performed. No dependencies were installed or upgraded.

Installed SVAR calendar 2.7.1 and editor 2.7.0 license files are MIT. The adapter imports free Calendar/Willow themes; recurrence is supplied as concrete Rust-expanded occurrences rather than PRO recurrence. Widget actions used by the application work in the successful real-WebKit benchmark. Exact package/license/source inspection is stronger evidence for the pinned build than assumptions about the latest upstream API. Comprehensive transitive GPL compatibility and notices in distributable packages were not independently certified; the project's dependency document is not a substitute for release-artifact license review.

### Frontend and Rust quality

Svelte runes and immutable/raw state boundaries are generally used coherently. The widget adapter isolates SVAR and uses its action bus for individual mutations rather than replacing the events prop on every edit. Filters support visibility/search without reconstructing domain events, and colors use a small stylesheet. Range requests use cancellation signals and obsolete-result checks. These are good design choices despite the race/cache findings.

Rust error types and crate boundaries are readable. Most `unwrap`/`expect` usage is in tests, fixed-value construction, or narrowly reasoned invariants; a raw count would be misleading. The greater reliability issue is unchecked arithmetic on remotely supplied values. Normal network/database operations are asynchronous, but CPU parsing and finite recurrence indexing still occur inline in some async write/sync paths. Detached scheduler task ownership and absent shutdown cancellation deserve explicit design.

Platform-neutral engine and Tauri/keyring choices preserve a plausible Windows/macOS path. Linux-only benchmark `/proc` access is appropriately guarded. Other operating systems, HiDPI, native keyrings, packages, suspend/resume, and multiple app instances were not tested. There is no evidence of a hard architectural need to abandon Linux-first design.

## 6. Validation record and reproducibility

### Existing checks executed

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --offline --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --locked --offline --workspace --all-features` with `UC_CALDAV_*` unset | 85 substantive tests passed; two optional real-server bodies skipped via early return, although the harness displays them as passing |
| Existing CalDAV and sync `real_server` tests against a fresh audit-only Radicale | Both passed; dummy credentials, random loopback port, temporary collections; server stopped afterward |
| `npm run check` | Passed, zero errors/warnings |
| `npm test` | 10 files, 96 tests passed, repository's Europe/Berlin timezone setting |
| `npm run build` | Passed; 432 transformed modules; main JS about 232 kB, 76 kB gzip |
| `cargo build --locked --offline --release -p universal-calendar-desktop --features tauri/custom-protocol` | Passed; rebuilt executable used for GUI measurements |
| `cargo run --locked --offline --release -p calendar-storage --example bench_queries` | Passed; measurements in section 4 |
| GUI benchmark, sizes 100/1,000/5,000 plus 30-second idle | Completed successfully; measurements in section 4 |
| `npm audit --json` | Two low findings; no automatic changes |
| Temporary adversarial Rust probes | 17 assertions of the documented erroneous states passed |
| Temporary frontend probes | Four checks reproduced DST timing change, status overwrite, stale load overwrite, and cache retention |

Toolchain: Rust/Cargo 1.99.1; Node 26.13.0. npm was initially absent from PATH but already installed under `/home/chris/.local/node-v26.13.0-linux-x64/bin`; checks used that existing installation. No installation was necessary.

The adversarial probe assertions deliberately expect the bad behavior to demonstrate its presence; they are **not passing correctness tests** and must be inverted when converted into regression tests. Application source and repository test files were not edited.

Temporary evidence available in this audit environment:

- `/tmp/uc-audit-probes-1g3fit7w/`: standalone Cargo project using local crate path dependencies, probe source, frontend Vite/SSR probe, npm audit JSON.
- `/tmp/uc-audit-bench-m3-brq7fwoj/results.json`: current GUI raw results and machine metadata; `stdout.txt` contains the benchmark output.
- `/tmp/uc-audit-radicale-6clkkksm/`: isolated server configuration/collections/log, not production data.

These temporary paths are not permanent project deliverables. The relevant outcomes, measurements, source locations, and reproduction steps are recorded in this report.

Example reproduction commands for the retained temporary probes:

```sh
CARGO_TARGET_DIR=/home/chris/calendar/target cargo test --offline \
  --manifest-path /tmp/uc-audit-probes-1g3fit7w/Cargo.toml -- --nocapture

TZ=Europe/Berlin /home/chris/.local/node-v26.13.0-linux-x64/bin/node \
  /tmp/uc-audit-probes-1g3fit7w/frontend.mjs
```

For a fresh reproduction, implement each finding's small sequence against `Store::open_in_memory()` and a dummy remote account; do not point tests at an existing calendar. The first-create/delete probe intentionally interleaves snapshot capture, local deletion and upload completion; the stale-load probe controls promise resolution. Neither requires an external service.

### Coverage limits and missing regression cases

The existing suite covers ordinary CRUD, read-only refusal, persistence/reopen, queue coalescing, an edit during upload, common recurrence, mock HTTP behavior, and basic conflict resolution. It does not adequately cover deleted-object conflict recovery, create/delete in flight, ambiguous submission/crash outcomes, transformed PUT responses, malformed replacement of previously valid data, structural preservation of overrides, RDATE-only rules, old hourly series, title-only DST-fold edits, auth-failure scheduler behavior, or local-save/load ordering.

No checked-in automated test exercises the actual CalendarAdapter's event bus, drag/resize, visibility, or interaction with async sync notifications. The scripted GUI benchmark exercises real widget behavior, but is not an assertion-based end-to-end correctness suite. Scheduler timing and task lifecycle have no dedicated tests. There are no checked-in CI gates, minimum-Rust checks, migration-upgrade fixtures for previously populated databases, or independent performance regression thresholds.

**Not executed/verified:** live Nextcloud (the named M3 reference provider), iCloud/Fastmail/Baïkal, Google/Graph, actual OS keyring storage, KDE/GPU/HiDPI benchmarks, true pointer-driven drag/resizing FPS, screen readers, offline network fault injection into a real server, power-loss recovery, app package installation, Windows/macOS, and full Rust/transitive license/advisory audits. No result is inferred for these environments from Radicale success.

## 7. Recommended order of follow-up work

1. **Restore integrity before broadening provider scope:** F01–F06 and F09; add failure/interleaving/structural-round-trip regression tests. Preserve raw data and deletion intent until outcomes are known.
2. **Close authentication and availability gaps:** F07, F10–F15; include failed tombstones, missing versions, account backoff, task supervision, and consistent UI lifecycle status.
3. **Make recurrence limitations explicit and safe:** F08 and F16–F18; use complete supported semantics or preserve unsupported objects without destructive edits.
4. **Establish target-machine performance evidence:** F19–F20, actual pointer interactions, full-load paint, and synchronization under input/persistence load. Keep the current fast storage baseline.
5. **Settle future-provider seams before M4:** F21, then align toolchain/CI, accessibility, and documentation (F22–F24).

Milestone 3 has a substantial working vertical slice, not a validated data-integrity guarantee. Nextcloud acceptance and the reproduced correctness failures remain open. No fixes, commits, dependency upgrades, or production-data changes were made during this audit.
