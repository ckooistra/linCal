# 0001: Use SVAR Svelte Calendar (MIT edition) behind an adapter

- **Date:** 2026-10-29
- **Status:** accepted for Milestone 1; revisit after a benchmark on real hardware

## Context

The spec asks for SVAR Svelte Calendar's free, MIT-licensed edition, with its license and features verified first, and with the widget kept replaceable.

## Verification (2026-10-29)

**Package and license**

- `@svar-ui/svelte-calendar` **2.7.1** (published 2026-09-15), license **MIT**, peer dependency `svelte ^5.54`.
- All `@svar-ui/*` dependencies are MIT. `@svar-ui/svelte-editor` declares "SEE LICENSE IN license.txt"; that file is the MIT license.

**Free edition, as stated in the upstream README and confirmed against the package types and source**

- Day, week and month views; drag to move, resize and create.
- Event editor; multiple calendars and filtering.
- Light and dark Willow themes; iCal import/export; REST provider.

**PRO only, not used**

- Year, agenda, timeline and resources views.
- Recurrence (RRULE), time-zone support, **dynamic loading**, undo/redo, PDF/Excel export.

The package contains `pro_*` files, such as the recurrence form. We don't import them.

## Findings that shaped the adapter

- **Prop changes rebuild the store.** Changing the `events` prop re-initialises SVAR's whole internal store. The adapter passes `events` once and then uses the action bus: `provide-data`, `update-event`, `add-event`, `delete-event` and `filter-events`.
- **Linear scans.** SVAR's internal `EventsStore` does a linear scan for every lookup, and `provide-data` with `reset: false` is O(n²). The adapter therefore always uses `reset: true` and only ever hands SVAR the visible range plus one month either side.
- **Intercepts.** `intercept()` returning `false` cancels an action. Moves, edits, creates and clicks are intercepted and routed through our `EventStore`. SVAR never owns data and read-only rules are enforced. Adapter-issued actions carry a private marker so the intercepts let them through.
- **CDN loading.** The Willow theme wrappers load fonts and icons from `cdn.svar.dev` by default. We pass `fonts={false}`, use the system font, and don't use SVAR's toolbar or editor, which are the only components that need its icon font. The CSP blocks any remote fetch regardless.
- **No calendar-colour prop.** Colours come from a generated stylesheet that sets `--wx-color-primary` per calendar class, through the `eventCss` hook.

## Consequences

- Recurrence and time zones must be implemented in the Rust core. That is the plan anyway (spec §9); SVAR only ever receives concrete occurrences.
- Range-limited loading is our store's job, since SVAR's dynamic loading is PRO-only.
- The agenda view will need a custom Svelte component (Milestone 5).
- Benchmarks show acceptable responsiveness at 1,000 events. At 5,000 events, rendering a dense week view is paint-bound in a software-rendered VM (see `docs/performance.md`). If this persists on real hardware, the adapter boundary lets us optimise the event markup or swap the widget.
