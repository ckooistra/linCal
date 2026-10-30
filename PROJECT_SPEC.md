# Project: Universal Calendar — Open-Source Linux Desktop Calendar

## 1. Project Overview

Build a modern, fast, responsive, free and open-source desktop calendar application for Linux.

The application's primary purpose is to provide a unified calendar interface capable of connecting to multiple calendar providers, including:

- Google Calendar
- Microsoft 365 / Outlook.com
- Nextcloud Calendar
- iCloud Calendar
- Fastmail
- Generic CalDAV servers
- ICS calendar subscriptions

The application must be **local-first**, operate without requiring a proprietary backend service, and remain fully usable when offline.

The primary development environment is Linux running KDE Plasma.

Although Linux is the initial target, the architecture should preserve the possibility of supporting Windows and macOS in the future.

### Primary Goals

1. Exceptional interface responsiveness.
2. Seamless integration with multiple calendar providers.
3. Modern, intuitive user experience.
4. Reliable offline functionality.
5. Efficient CPU and memory utilization.
6. Modular, maintainable architecture.
7. Completely free and open-source software.
8. Privacy-focused operation without requiring an application-specific account.

**Performance and interface responsiveness are first-class architectural requirements.**

---

# 2. Technology Stack

Use the following technologies unless a compelling technical reason exists to deviate.

| Component | Technology |
|---|---|
| Desktop framework | Tauri 2 |
| Frontend framework | Svelte 5 |
| Frontend language | TypeScript |
| Frontend build system | Vite |
| Calendar UI | SVAR Svelte Calendar (MIT-licensed free edition) |
| Backend language | Rust |
| Async runtime | Tokio |
| HTTP client | Reqwest |
| Local database | SQLite |
| Database access | SQLx |
| Date/time handling | Chrono + chrono-tz |
| Credentials | Linux Secret Service / system keyring |
| Initial operating system | Linux / KDE Plasma |
| Initial packaging | AppImage and/or native Linux packages |

Do not introduce SvelteKit unless a clear architectural need emerges.

Do not introduce a separate backend server.

The application must run entirely on the user's computer, communicating directly with the configured calendar providers.

## Open-Source Requirements

The application must remain completely free and open source.

Prefer a GPL-3.0-or-later license for application code, subject to verifying compatibility with dependencies and their licenses.

All essential features must be implemented using freely redistributable components.

Do not depend on:

- SVAR PRO.
- Commercial calendar widgets.
- Paid synchronization libraries.
- Proprietary backend services.
- Commercial authentication intermediaries.
- Paid APIs for essential application functionality.

The MIT-licensed edition of SVAR Calendar is acceptable.

Verify the current package's license and feature availability before integration.

If SVAR proves unsuitable, evaluate other fully open-source calendar components.

Keep calendar rendering behind a frontend abstraction so that the calendar component can be replaced without rewriting the application.

---

# 3. Primary Architectural Principle

**Separate the calendar engine from the desktop application.**

The Rust calendar engine must not depend directly on Tauri.

Tauri should act as a relatively thin integration layer between the frontend and Rust services.

This will allow the calendar engine to be reused by:

- The current desktop application.
- A potential command-line interface.
- Other desktop interfaces.
- Future applications.

Recommended project structure:

```text
universal-calendar/
├── apps/
│   └── desktop/
│       ├── src/
│       │   ├── components/
│       │   ├── views/
│       │   ├── stores/
│       │   ├── services/
│       │   ├── types/
│       │   └── App.svelte
│       │
│       ├── src-tauri/
│       │   ├── src/
│       │   │   ├── commands/
│       │   │   ├── state/
│       │   │   └── lib.rs
│       │   └── tauri.conf.json
│       │
│       ├── package.json
│       └── vite.config.ts
│
├── crates/
│   ├── calendar-core/
│   ├── calendar-storage/
│   ├── calendar-sync/
│   ├── calendar-provider-api/
│   ├── calendar-caldav/
│   ├── calendar-google/
│   └── calendar-microsoft/
│
├── docs/
│   ├── architecture.md
│   ├── provider-integration.md
│   ├── synchronization.md
│   └── performance.md
│
├── Cargo.toml
├── LICENSE
└── README.md
```

Adjust the directory structure if necessary, but preserve the modular boundaries.

---

# 4. Performance Requirements

## 4.1 Primary Objective

The application must feel immediately responsive during normal user interactions.

Users should never perceive delays caused by:

- Network requests.
- Calendar synchronization.
- Database writes.
- Authentication operations.
- Recurrence calculations.
- Loading unrelated calendars.

Use an asynchronous, local-first architecture.

## 4.2 Performance Targets

The following are development targets rather than guaranteed results.

| Operation | Target |
|---|---|
| Switching calendar months | Under 50 ms |
| Opening an event editor | Under 50 ms |
| Toggling calendar visibility | Under 30 ms |
| Displaying locally cached events | Under 100 ms |
| Searching locally cached events | Under 100 ms |
| Dragging/resizing events | Smooth 60 FPS |
| Creating an event | Immediate visual feedback |
| Idle CPU usage | As close to zero as practical |

Measure these targets on the development machine and document results.

Do not sacrifice correctness or reliability to meet arbitrary benchmarks.

## 4.3 Optimistic UI Updates

Use optimistic updates for user actions.

For example, when a user moves an event:

1. Immediately reflect the change in the Svelte interface.
2. Persist the modification locally.
3. Queue the change for synchronization.
4. Synchronize with the appropriate provider asynchronously.
5. Update the event's synchronization status.
6. Handle failures without silently losing the user's changes.

Network latency must not delay local interactions.

Distinguish between changes that are:

- Pending local persistence.
- Saved locally.
- Pending synchronization.
- Successfully synchronized.
- Failed to synchronize.
- In conflict.

## 4.4 Background Operations

All network operations must run independently of UI interaction.

Potentially expensive database and recurrence operations must not block the UI thread.

Use Tokio for asynchronous background operations.

Use bounded concurrency, appropriate cancellation, and dedicated blocking execution where required.

Avoid unnecessary polling or CPU-intensive background loops.

## 4.5 Efficient Calendar Rendering

Only retrieve events for the visible date range, plus a reasonable prefetch window.

Do not load an entire user's calendar history into the frontend.

Implement:

- Visible-range queries.
- Adjacent-month prefetching.
- Incremental event updates.
- Efficient event filtering.
- Debounced searching.
- Cached recurrence expansion.
- Stable event identifiers.
- Cancellation of obsolete loading requests.

Avoid unnecessary Svelte reactive updates.

During drag-and-drop interactions, maintain visual feedback entirely in the frontend. Send the final result to Rust when the interaction completes.

## 4.6 Performance Benchmarking

Create a repeatable performance test environment containing simulated calendar data.

Test at least:

- 100 events.
- 1,000 events.
- 5,000 events.
- Multiple simultaneous calendars.
- Multiple events on the same day.
- Overlapping events.
- Recurring event occurrences.
- Rapid month navigation.
- Repeated calendar visibility changes.

Measure:

- Interaction latency.
- Frame rate.
- Rendering performance.
- Memory consumption.
- CPU consumption.
- Database query latency.

Establish a performance baseline before implementing provider synchronization.

---

# 5. User Interface Requirements

The frontend must use Svelte 5 and TypeScript.

Use the current stable, open-source edition of SVAR Svelte Calendar initially.

Do not use commercially licensed SVAR features.

## Core Views

Implement:

### Month View

- Traditional month grid.
- Events displayed within each day.
- Multiple calendar colours.
- Today highlighting.
- Previous/next month navigation.
- Click to create an event.
- Click to edit an event.

### Week View

- Seven-day layout.
- Hourly time grid.
- All-day events.
- Overlapping event display.
- Drag-and-drop event movement.
- Event duration resizing where supported.

### Day View

- Detailed daily schedule.
- Time-based event layout.
- All-day event section.
- Event creation and editing.

### Agenda View

Implement later if not available in the free SVAR edition.

Use a custom Svelte component if required.

## Interface Design

The interface should be:

- Modern.
- Minimal.
- Responsive.
- Keyboard-friendly.
- Accessible.
- Suitable for high-density displays.
- Comfortable on KDE Plasma.

Implement light and dark themes.

Respect system theme preferences where practical.

Provide a sidebar for:

- Connected accounts.
- Available calendars.
- Calendar visibility.
- Calendar colours.
- Settings.

Avoid unnecessarily complicated navigation.

## Keyboard Navigation

Plan support for:

| Shortcut | Action |
|---|---|
| T | Jump to today |
| N | Create new event |
| M | Month view |
| W | Week view |
| D | Day view |
| Ctrl+F | Search |
| Escape | Close dialog |

Shortcuts must not interfere with typing in form fields.

Global operating-system shortcuts may be added later with user-configurable bindings.

---

# 6. Unified Calendar Data Model

Design a provider-independent calendar model.

The frontend must not need to understand provider-specific event formats.

Include models for:

- Account.
- Calendar.
- Calendar event.
- Event occurrence.
- Attendee.
- Reminder.
- Recurrence rule.
- Synchronization state.
- Pending operation.
- Provider capabilities.

At minimum, events should support:

```text
Event
├── Internal ID
├── Provider ID
├── Provider Event ID
├── Calendar ID
├── Title
├── Description
├── Location
├── Start Date/Time
├── End Date/Time
├── Time Zone
├── All-Day Flag
├── Recurrence Rule
├── Recurrence Exceptions
├── Attendees
├── Reminders
├── Organizer
├── Availability Status
├── Visibility
├── Provider Version / ETag
├── Sync Status
├── Created Timestamp
└── Updated Timestamp
```

This structure is illustrative.

Design appropriate database tables and Rust types rather than implementing this literally as one large object.

## Important Data Considerations

Handle:

- UTC timestamps.
- Local time zones.
- All-day dates.
- Daylight-saving transitions.
- Floating local times.
- Recurring events.
- Recurrence exceptions.
- Event cancellation.
- Provider-specific IDs.
- Provider-specific metadata.
- Duplicate detection.
- Events belonging to multiple accounts.

Do not assume all providers support identical features.

Preserve provider-specific information where necessary to avoid destructive round-trip conversions.

---

# 7. Provider Architecture

Implement calendar providers through a common Rust interface.

Suggested conceptual interface:

```rust
#[async_trait::async_trait]
pub trait CalendarProvider: Send + Sync {
    async fn list_calendars(
        &self,
    ) -> Result<Vec<Calendar>, CalendarError>;

    async fn fetch_changes(
        &self,
        calendar: &Calendar,
        cursor: Option<&str>,
    ) -> Result<SyncChanges, CalendarError>;

    async fn create_event(
        &self,
        event: &CalendarEvent,
    ) -> Result<CalendarEvent, CalendarError>;

    async fn update_event(
        &self,
        event: &CalendarEvent,
    ) -> Result<CalendarEvent, CalendarError>;

    async fn delete_event(
        &self,
        event: &CalendarEvent,
    ) -> Result<(), CalendarError>;
}
```

This interface is a starting point.

Improve it to accommodate:

- Provider authentication.
- Account initialization.
- Capability discovery.
- Incremental synchronization.
- Pagination.
- Rate limiting.
- Version tracking.
- Provider-specific constraints.
- Read-only calendars.

## Initial Provider Support

### CalDAV

Highest initial priority.

Support:

- Nextcloud.
- Generic CalDAV servers.
- iCloud where authentication permits.
- Fastmail.
- Radicale.
- Baïkal.

Implement CalDAV discovery, calendar listing, event retrieval, creation, updating, and deletion.

Support ETag-based conditional updates and synchronization tokens where available.

Handle unsupported server capabilities gracefully.

### Google Calendar

Use the official Google Calendar API.

Implement OAuth authentication appropriate for an installed desktop application.

Support incremental synchronization with sync tokens.

Handle token expiration, invalidation, pagination, and API rate limits.

### Microsoft Calendar

Use Microsoft Graph API.

Support Microsoft personal and work/school accounts where permitted by their authentication and organizational policies.

Implement OAuth authentication using an appropriate public-client flow.

Support calendar listing, events, recurring events, and incremental synchronization.

Account for Microsoft Graph's calendar-view delta synchronization semantics.

### ICS Subscriptions

Support read-only calendar subscriptions.

Implement:

- URL-based subscriptions.
- Periodic refresh.
- ETag and Last-Modified handling where supported.
- Importing local ICS files.

Do not assume subscriptions support event modification.

---

# 8. Synchronization Engine

The synchronization engine must be independent of Tauri and the frontend.

## Synchronization Requirements

Implement:

1. Initial account synchronization.
2. Incremental synchronization.
3. Persistent synchronization state.
4. Local operation queue.
5. Automatic retry.
6. Exponential backoff.
7. Rate-limit handling.
8. Offline detection.
9. Conflict detection.
10. Recovery after application crashes.

Never silently discard local changes.

Do not overwrite remote modifications without detecting relevant conflicts.

## Synchronization Flow

```text
Remote Calendar Providers
            │
            ▼
    Provider Adapters
            │
            ▼
   Background Sync Engine
            │
            ▼
       Local SQLite
            │
            ▼
       Tauri Commands
            │
            ▼
      Svelte Frontend
```

Local user modifications should be persisted before attempting remote synchronization.

Implement durable pending operations so that changes survive application restarts.

Where a provider supports conditional updates, use its version tokens or ETags to prevent accidental overwrites.

Handle duplicate submissions, partial failures, and ambiguous network outcomes carefully.

The sync engine must be designed to avoid losing changes when the application closes unexpectedly.

---

# 9. Recurring Events

Recurring events are essential.

Do not rely on paid SVAR recurrence functionality.

Implement recurrence handling within the Rust calendar core.

Support relevant iCalendar recurrence concepts, including:

- RRULE.
- RDATE.
- EXDATE.
- RECURRENCE-ID.
- Recurrence exceptions.
- Time-zone-aware recurrence calculations.

The architecture must eventually support editing:

1. A single occurrence.
2. An entire series.
3. This and future occurrences.

Be mindful that editing future occurrences can require provider-specific series-splitting behaviour.

Expand recurring events only for the requested date range.

Cache expanded occurrences where useful.

Include automated tests for:

- Daily recurrence.
- Weekly recurrence.
- Monthly recurrence.
- Yearly recurrence.
- Daylight-saving transitions.
- Recurrence exceptions.
- Modified individual occurrences.
- Deleted occurrences.
- Recurrence rules without end dates.

Do not write a simplistic recurrence engine that assumes recurrence means adding a fixed number of seconds to a timestamp.

Prefer evaluating established open-source Rust libraries implementing the relevant iCalendar standards before implementing recurrence parsing and expansion manually.

---

# 10. Local Database

Use SQLite as the application's local database.

Create schema migrations.

Potential tables include:

```text
accounts
calendars
events
event_occurrences
attendees
reminders
sync_state
pending_operations
provider_metadata
application_settings
```

Refine this schema as appropriate.

## Database Requirements

- Use transactions for related operations.
- Implement appropriate indexes.
- Consider WAL mode.
- Use prepared statements.
- Avoid unnecessary full-table scans.
- Support schema migrations.
- Preserve user data across application upgrades.
- Keep application settings separate from secrets.

Optimize event queries around visible date ranges and calendar IDs.

For recurring events, avoid an unbounded occurrence table.

Use bounded expansion and caching where appropriate.

---

# 11. Authentication and Security

Authentication must be secure and provider-appropriate.

Use OAuth 2.0 with PKCE or another officially supported native-client authorization flow for providers requiring OAuth.

Do not embed confidential client secrets in the distributed desktop executable.

Use the operating system's secure credential storage wherever possible.

On KDE Plasma, support the desktop's available Secret Service implementation or another suitable secure credential backend.

Never store access tokens or passwords in plaintext SQLite tables.

Do not log authentication secrets.

Do not implement a custom authentication server unless absolutely necessary.

The application must work without an account on any service operated by the application developer.

Document provider registration requirements clearly.

---

# 12. Error Handling and Reliability

The application should handle common failures gracefully.

Examples:

- Internet connection unavailable.
- OAuth token expired.
- Provider temporarily unavailable.
- Account authentication revoked.
- Rate limits exceeded.
- Malformed calendar data.
- Unsupported calendar features.
- Conflicting event modifications.
- Calendar deleted remotely.
- SQLite temporarily unavailable.

Do not display technical stack traces to end users.

Provide understandable error messages.

Maintain structured application logs for troubleshooting, with sensitive fields redacted.

Ensure failures in one provider do not prevent other accounts from functioning.

---

# 13. Testing Strategy

Implement automated testing throughout development.

## Rust Tests

Cover:

- Event validation.
- Date and time handling.
- Recurrence expansion.
- Provider-independent data models.
- SQLite operations.
- Pending operation persistence.
- Synchronization logic.
- Conflict detection.
- Provider response parsing.

## Frontend Tests

Cover:

- Calendar navigation.
- Event rendering.
- Event creation.
- Event editing.
- Calendar visibility.
- Keyboard navigation.
- Optimistic updates.
- Loading and error states.

## Integration Tests

Use mock provider servers or recorded fixtures where appropriate.

Do not require real Google or Microsoft credentials for ordinary automated tests.

Add optional integration tests for a developer-provided CalDAV server.

## Performance Tests

Maintain a deterministic event-data generator.

Create reproducible benchmarks for calendar rendering and local queries.

Document measured performance and regressions.

---

# 14. Development Milestones

> **Status (October 2026):**
> - Done: Milestones 1–3. CalDAV is validated against Nextcloud and the October audit's findings are addressed.
> - In progress: Milestone 4.

Implement the project incrementally.

## Milestone 1 — Desktop Foundation and Performance Prototype

**This is the immediate implementation priority.**

Build:

- Tauri 2 application.
- Svelte 5 frontend.
- TypeScript configuration.
- Vite build system.
- SVAR Calendar integration.
- Month, week, and day views.
- Basic navigation.
- Light/dark theme support.
- Mock event data.
- Calendar visibility toggles.
- Basic event creation/editing using mock data.
- Initial performance benchmarking.

Use realistic simulated calendar data.

The application must build and run on Linux/KDE Plasma.

At this stage, no remote calendar integrations are required.

### Milestone 1 Acceptance Criteria

- Application launches successfully.
- Calendar renders correctly.
- Month/week/day navigation works.
- Events can be displayed and manipulated.
- Multiple simulated calendars are supported.
- UI remains responsive with 1,000 simulated events.
- Basic performance measurements are documented.
- No commercial dependencies are used.

**Complete and validate this milestone before beginning provider integrations.**

---

## Milestone 2 — Rust Calendar Core and SQLite

Implement:

- Independent Rust calendar models.
- SQLite storage.
- Database migrations.
- Tauri commands.
- Local event CRUD operations.
- Visible-range event queries.
- Optimistic frontend updates.
- Persistent application settings.
- Initial recurrence support.

Replace mock event storage with SQLite.

### Acceptance Criteria

- Events persist across application restarts.
- Local event operations remain responsive.
- Event queries are restricted to relevant date ranges.
- The Rust calendar core builds independently of Tauri.

---

## Milestone 3 — CalDAV Integration

Implement:

- CalDAV account setup.
- Server discovery.
- Authentication.
- Calendar listing.
- Event retrieval.
- Event creation.
- Event modification.
- Event deletion.
- Synchronization.
- Offline operation queue.

Use Nextcloud as the primary reference provider.

Do not hardcode Nextcloud-specific behaviour into the generic CalDAV adapter.

### Acceptance Criteria

- A Nextcloud account can be connected.
- Calendars appear in the sidebar.
- Events synchronize correctly.
- Local changes synchronize to Nextcloud.
- Remote changes appear locally.
- Offline changes survive application restarts.
- Synchronization failures do not freeze the interface.

---

## Milestone 4 — Google and Microsoft

Implement:

- Google OAuth authentication.
- Google Calendar API integration.
- Microsoft OAuth authentication.
- Microsoft Graph integration.
- Incremental synchronization.
- Account management.
- Multiple accounts from the same provider.

### Acceptance Criteria

- Google and Microsoft calendars can coexist.
- Events from different providers display together.
- Account-specific errors are isolated.
- Synchronization does not block local interaction.
- Provider differences are hidden behind the unified calendar model.

---

## Milestone 5 — Production Quality

Implement:

- Comprehensive recurring-event functionality.
- Recurrence exception editing.
- Advanced conflict handling.
- Desktop notifications.
- Search.
- Agenda view.
- ICS subscriptions.
- Keyboard shortcuts.
- Accessibility improvements.
- AppImage packaging.
- Additional Linux packaging.
- User documentation.
- Performance profiling and optimization.

---

# 15. Documentation Requirements

Maintain the following documentation:

### README.md

Include:

- Project overview.
- Features.
- Supported platforms.
- Screenshots when available.
- Development prerequisites.
- Build instructions.
- Running instructions.
- Testing instructions.
- License.

### docs/architecture.md

Explain:

- Module boundaries.
- Rust crate responsibilities.
- Tauri integration.
- Frontend architecture.
- Data flow.

### docs/provider-integration.md

Explain:

- Provider interface.
- Authentication requirements.
- Provider capabilities.
- How to add a new calendar provider.

### docs/synchronization.md

Explain:

- Initial synchronization.
- Incremental synchronization.
- Offline changes.
- Conflict resolution.
- Failure recovery.

### docs/performance.md

Document:

- Performance targets.
- Benchmark procedures.
- Test datasets.
- Measured results.
- Known bottlenecks.
- Future optimizations.

---

# 16. Coding Standards

## Rust

- Follow idiomatic Rust conventions.
- Use `cargo fmt`.
- Use `cargo clippy`.
- Prefer explicit error types.
- Avoid unnecessary cloning.
- Avoid blocking async executors.
- Keep public interfaces small and documented.
- Include unit tests for important logic.

## TypeScript and Svelte

- Enable strict TypeScript checking.
- Use Svelte 5 conventions.
- Prefer small, focused components.
- Avoid unnecessary global state.
- Avoid excessive reactive recomputation.
- Keep provider-specific logic out of UI components.
- Properly clean up listeners and timers.
- Handle asynchronous operations and cancellation carefully.

## General

- Prefer simple solutions over unnecessary abstractions.
- Avoid premature microservice-style architecture.
- Do not introduce a backend server.
- Do not add analytics or telemetry by default.
- Do not introduce paid dependencies.
- Avoid large unnecessary dependencies.
- Keep the application easy to build and maintain.

---

# 17. Instructions for the Coding Agent

You are acting as the lead software engineer responsible for implementing this project.

Work autonomously and make reasonable architectural decisions.

Follow these instructions:

1. Inspect the repository and development environment before making changes.
2. Verify the compatibility and licensing of major dependencies.
3. Establish the project structure.
4. Create the foundational documentation.
5. Implement Milestone 1 first.
6. Run relevant builds, tests, and linting.
7. Fix errors rather than leaving a broken project.
8. Benchmark the interface using simulated data.
9. Document decisions and trade-offs.
10. Provide a concise implementation summary when the milestone is complete.

Do not attempt to implement every milestone simultaneously.

Do not add unnecessary abstractions merely because future features are planned.

However, preserve the architectural boundaries needed for multiple provider integrations.

**Prioritize a working, polished, exceptionally responsive calendar interface over implementing a large number of incomplete features.**

When making technical decisions, prioritize in this order:

1. Data correctness and protection against data loss.
2. Interface responsiveness.
3. Reliability.
4. Maintainability.
5. Privacy and security.
6. Low resource consumption.
7. Feature completeness.

Security and privacy are mandatory requirements, not optional optimizations; the ordering above is intended to guide trade-offs between otherwise acceptable implementations.

If an architectural decision affects the ability to support Google Calendar, Microsoft Graph, or CalDAV, document the implications before proceeding.

## Immediate Task

**Begin implementation of Milestone 1: Desktop Foundation and Performance Prototype.**

Create a functional Tauri 2 + Svelte 5 + TypeScript application using the free, MIT-licensed SVAR Calendar component.

Implement simulated calendars and events, including a deterministic generator for performance testing.

Provide month, week, and day views, navigation, event interaction, multiple calendars, and light/dark theme support.

Ensure the application can be built and launched on Linux running KDE Plasma.

Run the available tests and build checks.

Measure initial interface responsiveness and document the results.

Do not implement remote calendar provider integrations yet.

The objective of this milestone is to validate that the selected frontend technology can deliver the responsiveness expected from a high-quality native desktop calendar application.
