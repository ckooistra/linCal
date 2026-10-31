import type {
  Account,
  Calendar,
  CalendarEvent,
  DateRange,
  EditScope,
  EventDraft,
  EventId,
} from "../types/calendar";
import { invoke } from "./bridge";
import type { EventSource, SearchOptions } from "./eventSource";
import { localTimeZone } from "./time";

function abortError(): DOMException {
  return new DOMException("The operation was aborted.", "AbortError");
}

/**
 * `EventSource` backed by the Rust engine and SQLite through Tauri commands.
 * The viewer's IANA zone is sent with every call so all-day and floating
 * events resolve exactly as the UI renders them.
 */
export class TauriEventSource implements EventSource {
  readonly timeZone: string;

  constructor(timeZone: string = localTimeZone()) {
    this.timeZone = timeZone;
  }

  listAccounts(): Promise<Account[]> {
    return invoke<Account[]>("list_accounts");
  }

  listCalendars(): Promise<Calendar[]> {
    return invoke<Calendar[]>("list_calendars");
  }

  async queryRange(range: DateRange, options: { signal?: AbortSignal } = {}): Promise<CalendarEvent[]> {
    const { signal } = options;
    if (signal?.aborted) throw abortError();
    // IPC calls cannot be cancelled; an aborted request's result is dropped.
    const result = await invoke<CalendarEvent[]>("query_range", {
      range: { start: range.start, end: range.end },
      timeZone: this.timeZone,
    });
    if (signal?.aborted) throw abortError();
    return result;
  }

  async search(text: string, range: DateRange, options: SearchOptions): Promise<CalendarEvent[]> {
    const { signal, limit, latest = false } = options;
    if (signal?.aborted) throw abortError();
    const result = await invoke<CalendarEvent[]>("search_events", {
      text,
      range: { start: range.start, end: range.end },
      timeZone: this.timeZone,
      limit,
      latest,
    });
    if (signal?.aborted) throw abortError();
    return result;
  }

  createEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent> {
    return invoke<CalendarEvent>("create_event", { id, draft, timeZone: this.timeZone });
  }

  updateEvent(id: EventId, draft: EventDraft, scope: EditScope, newId?: EventId): Promise<CalendarEvent> {
    return invoke<CalendarEvent>("update_event", { id, draft, scope, timeZone: this.timeZone, newId: newId ?? null });
  }

  deleteEvent(id: EventId, scope: EditScope): Promise<void> {
    return invoke<void>("delete_event", { id, scope });
  }
}
