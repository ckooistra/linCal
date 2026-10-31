/**
 * Provider-neutral calendar model used by the UI.
 *
 * These types deliberately avoid provider specifics (Google, Graph, CalDAV).
 * They mirror the serialized types in `crates/calendar-core/src/wire.rs`.
 */

export type AccountId = string;
export type CalendarId = string;
export type EventId = string;

/** Provider kinds the architecture plans for. Only `local` and `simulated` exist in M1. */
export type ProviderKind = "local" | "simulated" | "caldav" | "google" | "microsoft" | "ics";

export interface Account {
  id: AccountId;
  name: string;
  provider: ProviderKind;
}

export interface Calendar {
  id: CalendarId;
  accountId: AccountId;
  name: string;
  /** CSS colour, `#rrggbb`. */
  color: string;
  /** Read-only calendars (subscriptions, holidays, shared) reject edits. */
  readOnly: boolean;
}

/**
 * Lifecycle of a local change (spec §4.3). Milestone 1 stops at `saved`,
 * the sync states are modelled so the UI can already render them.
 */
export type SyncStatus =
  | "pending-local"
  | "saved"
  | "pending-sync"
  | "synced"
  | "sync-failed"
  | "conflict";

/** Calendar date without time, `YYYY-MM-DD`. Used for all-day events. */
export type IsoDate = string;

/**
 * When an event happens. Timed events are absolute instants (UTC epoch ms)
 * plus the IANA zone they were authored in (`null` = floating local time).
 * All-day events are calendar dates, end exclusive, and never converted
 * through a time zone, so they cannot shift across DST or zone changes.
 */
export type EventTiming =
  | { kind: "timed"; start: number; end: number; timeZone: string | null }
  | { kind: "all-day"; startDate: IsoDate; endDate: IsoDate };

/**
 * One displayed occurrence. For a one-off event `id` is the event id; for an
 * occurrence of a recurring series it is `<seriesId>::<recurrenceId>`, which
 * stays stable when that occurrence is moved or edited on its own.
 */
export interface CalendarEvent {
  id: EventId;
  calendarId: CalendarId;
  title: string;
  description: string;
  location: string;
  timing: EventTiming;
  /** Recurring series this occurrence belongs to, if any. */
  seriesId: string | null;
  /** Original start of this occurrence within its series (RECURRENCE-ID). */
  recurrenceId: string | null;
  /** RRULE of the series (e.g. `FREQ=WEEKLY;BYDAY=MO`), if recurring. */
  rrule: string | null;
  syncStatus: SyncStatus;
  /** Monotonic revision, bumped on every change. */
  revision: number;
}

/** Fields a user can edit. `rrule` applies to whole events/series only. */
export type EventDraft = Pick<
  CalendarEvent,
  "calendarId" | "title" | "description" | "location" | "timing"
> & { rrule?: string | null };

/**
 * What an edit or delete applies to: a one-off event, a single occurrence
 * of a series, or the whole series.
 */
export type EditScope = "event" | "occurrence" | "series";

export function isRecurring(event: Pick<CalendarEvent, "seriesId">): boolean {
  return event.seriesId !== null;
}

/** Sync state of an account, as reported by the engine. */
export interface AccountStatus {
  accountId: AccountId;
  /** True for accounts synchronized with a server. */
  remote: boolean;
  lastSyncedAt: number | null;
  /** Last sync error shown to the user, or null. */
  error: string | null;
  pending: number;
  failed: number;
  conflicts: number;
  /** Server events whose newest version could not be read. */
  importFailures: number;
  /** Server events shown only approximately (e.g. unknown time zone). */
  approximated: number;
  /** An OAuth sign-in expired or was revoked. */
  needsSignIn: boolean;
}

/** How to settle a change the server refused or that conflicts. */
export type Resolution = "retry" | "keep-local" | "use-server";

/** A failed or conflicting change, including deletions (which have no event left). */
export interface SyncProblem {
  accountId: AccountId;
  calendarId: CalendarId;
  href: string;
  title: string;
  deletion: boolean;
  conflict: boolean;
  message: string;
}

/** Progress events from the background sync engine. */
export type SyncEvent =
  | { type: "syncing"; accountId: AccountId }
  | { type: "synced"; accountId: AccountId; changed: boolean }
  | { type: "failed"; accountId: AccountId; message: string; offline: boolean };

/** Half-open instant range `[start, end)` in epoch ms. */
export interface DateRange {
  start: number;
  end: number;
}

/** Views drawn by the calendar grid widget. */
export type CalendarViewKind = "month" | "week" | "day";

/** Every view: the grid views plus the agenda list. */
export type AppView = CalendarViewKind | "agenda";
