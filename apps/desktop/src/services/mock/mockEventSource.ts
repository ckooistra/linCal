import type {
  Account,
  Calendar,
  CalendarEvent,
  DateRange,
  EventDraft,
  EventId,
} from "../../types/calendar";
import { SourceError, type EventSource } from "../eventSource";
import { eventBounds } from "../time";
import type { Dataset } from "./generator";

export interface MockSourceOptions {
  /** Artificial delay applied to every call, to exercise loading states. */
  latencyMs?: number;
}

interface Entry {
  start: number;
  end: number;
  event: CalendarEvent;
}

/**
 * In-memory `EventSource` with an index sorted by start time.
 *
 * Range queries binary-search the start index and only scan back as far as
 * the longest event, so cost scales with the result size rather than the
 * dataset. Write failures can be injected to test optimistic rollback.
 */
export class MockEventSource implements EventSource {
  latencyMs: number;
  /** Number of upcoming write calls that should fail. */
  failNextWrites = 0;

  readonly #accounts: Account[];
  readonly #calendars: Calendar[];
  readonly #byId = new Map<EventId, Entry>();
  #sorted: Entry[] = [];
  #maxDuration = 0;

  constructor(dataset: Dataset, options: MockSourceOptions = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    this.#accounts = dataset.accounts;
    this.#calendars = dataset.calendars;
    for (const event of dataset.events) {
      const entry = this.#entry(event);
      this.#byId.set(event.id, entry);
      this.#sorted.push(entry);
    }
    this.#sorted.sort((a, b) => a.start - b.start);
  }

  get size(): number {
    return this.#byId.size;
  }

  async listAccounts(): Promise<Account[]> {
    await this.#delay();
    return this.#accounts.map((a) => ({ ...a }));
  }

  async listCalendars(): Promise<Calendar[]> {
    await this.#delay();
    return this.#calendars.map((c) => ({ ...c }));
  }

  async queryRange(range: DateRange, options: { signal?: AbortSignal } = {}): Promise<CalendarEvent[]> {
    await this.#delay(options.signal);
    return this.queryRangeSync(range);
  }

  /** Synchronous query, exposed for benchmarks and tests. */
  queryRangeSync(range: DateRange): CalendarEvent[] {
    const sorted = this.#sorted;
    const from = lowerBound(sorted, range.start - this.#maxDuration);
    const to = lowerBound(sorted, range.end);
    const result: CalendarEvent[] = [];
    for (let i = from; i < to; i++) {
      const entry = sorted[i]!;
      if (entry.end > range.start) result.push(entry.event);
    }
    return result;
  }

  async createEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent> {
    await this.#delay();
    this.#maybeFail();
    if (this.#byId.has(id)) throw new SourceError("An event with this id already exists.");
    this.#requireWritable(draft.calendarId);
    const event: CalendarEvent = {
      ...draft,
      id,
      seriesId: null,
      syncStatus: "saved",
      revision: 1,
      updatedAt: Date.now(),
    };
    this.#insert(this.#entry(event));
    return event;
  }

  async updateEvent(id: EventId, draft: EventDraft): Promise<CalendarEvent> {
    await this.#delay();
    this.#maybeFail();
    const existing = this.#byId.get(id);
    if (!existing) throw new SourceError("The event no longer exists.");
    this.#requireWritable(existing.event.calendarId);
    this.#requireWritable(draft.calendarId);
    const event: CalendarEvent = {
      ...existing.event,
      ...draft,
      syncStatus: "saved",
      revision: existing.event.revision + 1,
      updatedAt: Date.now(),
    };
    this.#remove(existing);
    this.#insert(this.#entry(event));
    return event;
  }

  async deleteEvent(id: EventId): Promise<void> {
    await this.#delay();
    this.#maybeFail();
    const existing = this.#byId.get(id);
    if (!existing) return;
    this.#requireWritable(existing.event.calendarId);
    this.#remove(existing);
  }

  #entry(event: CalendarEvent): Entry {
    const { start, end } = eventBounds(event);
    this.#maxDuration = Math.max(this.#maxDuration, end - start);
    return { start, end, event };
  }

  #insert(entry: Entry): void {
    this.#byId.set(entry.event.id, entry);
    this.#sorted.splice(upperBound(this.#sorted, entry.start), 0, entry);
  }

  #remove(entry: Entry): void {
    this.#byId.delete(entry.event.id);
    const index = this.#sorted.indexOf(entry, lowerBound(this.#sorted, entry.start));
    if (index !== -1) this.#sorted.splice(index, 1);
  }

  #requireWritable(calendarId: string): void {
    const calendar = this.#calendars.find((c) => c.id === calendarId);
    if (!calendar) throw new SourceError("The calendar no longer exists.");
    if (calendar.readOnly) throw new SourceError(`“${calendar.name}” is read-only.`);
  }

  #maybeFail(): void {
    if (this.failNextWrites > 0) {
      this.failNextWrites--;
      throw new SourceError("Simulated storage failure.");
    }
  }

  #delay(signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) return Promise.reject(abortError());
    if (this.latencyMs <= 0) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        signal?.removeEventListener("abort", onAbort);
        resolve();
      }, this.latencyMs);
      const onAbort = () => {
        clearTimeout(timer);
        reject(abortError());
      };
      signal?.addEventListener("abort", onAbort, { once: true });
    });
  }
}

function abortError(): DOMException {
  return new DOMException("The operation was aborted.", "AbortError");
}

/** First index whose start is >= value. */
function lowerBound(entries: Entry[], value: number): number {
  let lo = 0;
  let hi = entries.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (entries[mid]!.start < value) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** First index whose start is > value. */
function upperBound(entries: Entry[], value: number): number {
  let lo = 0;
  let hi = entries.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (entries[mid]!.start <= value) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
