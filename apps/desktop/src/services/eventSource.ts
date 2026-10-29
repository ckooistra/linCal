import type {
  Account,
  Calendar,
  CalendarEvent,
  DateRange,
  EventDraft,
  EventId,
} from "../types/calendar";

/**
 * Boundary between the UI and whatever owns event data.
 *
 * Milestone 1 implements it with an in-memory mock. Milestone 2 replaces it
 * with a thin wrapper around Tauri commands backed by the Rust core and
 * SQLite; stores and components only ever see this interface.
 */
export interface EventSource {
  listAccounts(): Promise<Account[]>;
  listCalendars(): Promise<Calendar[]>;
  /** Events overlapping `[range.start, range.end)`. Rejects with `AbortError` when aborted. */
  queryRange(range: DateRange, options?: { signal?: AbortSignal }): Promise<CalendarEvent[]>;
  /** Create with a client-generated stable id so optimistic UI and persistence agree. */
  createEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent>;
  updateEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent>;
  deleteEvent(id: EventId): Promise<void>;
}

/** User-facing error from a data source. */
export class SourceError extends Error {
  override name = "SourceError";
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}
