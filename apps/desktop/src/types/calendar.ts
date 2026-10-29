/**
 * Provider-neutral calendar model used by the UI.
 *
 * These types deliberately avoid provider specifics (Google, Graph, CalDAV).
 * In Milestone 2 they will mirror the Rust `calendar-core` types that cross
 * the Tauri bridge; for Milestone 1 they are populated by the mock source.
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

export interface CalendarEvent {
  /** Stable internal identifier; never reused. */
  id: EventId;
  calendarId: CalendarId;
  title: string;
  description: string;
  location: string;
  timing: EventTiming;
  /**
   * Identifier of the recurring series this occurrence belongs to. In M1 the
   * generator emits pre-expanded occurrences; real RRULE expansion moves to
   * the Rust core in Milestone 2.
   */
  seriesId: string | null;
  syncStatus: SyncStatus;
  /** Monotonic local revision, bumped on every local change. */
  revision: number;
  updatedAt: number;
}

/** Fields a user can edit. */
export type EventDraft = Pick<
  CalendarEvent,
  "calendarId" | "title" | "description" | "location" | "timing"
>;

/** Half-open instant range `[start, end)` in epoch ms. */
export interface DateRange {
  start: number;
  end: number;
}

export type CalendarViewKind = "month" | "week" | "day";
