import type {
  Account,
  Calendar,
  CalendarEvent,
  DateRange,
  EditScope,
  EventDraft,
  EventId,
} from "../types/calendar";

/**
 * Boundary between the UI and whatever owns event data.
 *
 * In the desktop app this is `TauriEventSource` (Rust core + SQLite); the
 * in-memory `MockEventSource` serves tests and the browser preview. Stores
 * and components only ever see this interface.
 */
export interface EventSource {
  listAccounts(): Promise<Account[]>;
  listCalendars(): Promise<Calendar[]>;
  /**
   * Occurrences overlapping `[range.start, range.end)`, with recurring
   * series expanded. Rejects with `AbortError` when aborted.
   */
  queryRange(range: DateRange, options?: { signal?: AbortSignal }): Promise<CalendarEvent[]>;
  /**
   * Occurrences in `range` whose title, location or description contain
   * every word of `text` (ignoring case), sorted by start. At most
   * `options.limit` are returned: the earliest, or with `latest` the latest.
   */
  search(text: string, range: DateRange, options: SearchOptions): Promise<CalendarEvent[]>;
  /** Create with a client-generated stable id so optimistic UI and persistence agree. */
  createEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent>;
  /**
   * Update a one-off event, a single occurrence, or a whole series (`id` is
   * then any occurrence id of it). Series updates return the series' first
   * occurrence; callers should reload affected ranges.
   */
  updateEvent(id: EventId, draft: EventDraft, scope: EditScope): Promise<CalendarEvent>;
  deleteEvent(id: EventId, scope: EditScope): Promise<void>;
}

export interface SearchOptions {
  limit: number;
  latest?: boolean;
  signal?: AbortSignal;
}

/** Search words of `text`, lower-cased; empty when there is nothing to search. */
export function searchTerms(text: string): string[] {
  return text.toLowerCase().split(/\s+/).filter(Boolean);
}

/** Whether `event` contains every term (see `searchTerms`). */
export function matchesSearch(
  event: Pick<CalendarEvent, "title" | "location" | "description">,
  terms: string[],
): boolean {
  const haystack = `${event.title}\n${event.location}\n${event.description}`.toLowerCase();
  return terms.every((term) => haystack.includes(term));
}

/** User-facing error from a data source. */
export class SourceError extends Error {
  override name = "SourceError";
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === "AbortError";
}
