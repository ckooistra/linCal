import { isAbortError, SourceError, type EventSource } from "../services/eventSource";
import { eventBounds, overlaps, validateTiming } from "../services/time";
import type {
  CalendarEvent,
  CalendarId,
  DateRange,
  EditScope,
  EventDraft,
  EventId,
} from "../types/calendar";
import { mergeRange, rangeCovered } from "./ranges";

export type StoreChange =
  | { type: "upsert"; event: CalendarEvent }
  | { type: "remove"; id: EventId }
  | { type: "loaded"; range: DateRange }
  /** Cached ranges were dropped (e.g. a whole series changed); reload what is visible. */
  | { type: "invalidated" }
  | { type: "reset" };

/** For occurrences of a series: change just this one, or the whole series. */
export type SeriesScope = "occurrence" | "series";

export type MutationResult = { ok: true; id: EventId } | { ok: false; message: string };

export type LoadPriority = "visible" | "prefetch";

export interface EventStoreOptions {
  isReadOnly: (calendarId: CalendarId) => boolean;
  onError: (message: string) => void;
  newId?: () => EventId;
}

/**
 * Client-side cache of events for the ranges the user has looked at.
 *
 * Events live in a plain (non-reactive) map; consumers subscribe to a change
 * feed and apply incremental updates, so editing one event never rebuilds
 * the whole view. Writes are optimistic: the change is visible immediately,
 * persisted through the `EventSource`, and rolled back with a notification
 * if persistence fails. Only `loading`/`loadError` are reactive.
 */
export class EventStore {
  loading = $state(false);
  loadError = $state<string | null>(null);

  #source: EventSource;
  readonly #options: EventStoreOptions;
  readonly #events = new Map<EventId, CalendarEvent>();
  /** Last state known to be persisted, used for rollback. */
  readonly #confirmed = new Map<EventId, CalendarEvent>();
  /** Deletions in flight; loads must not resurrect them. */
  readonly #tombstones = new Set<EventId>();
  readonly #listeners = new Set<(change: StoreChange) => void>();
  #loaded: DateRange[] = [];
  readonly #controllers: Record<LoadPriority, AbortController | null> = {
    visible: null,
    prefetch: null,
  };

  constructor(source: EventSource, options: EventStoreOptions) {
    this.#source = source;
    this.#options = options;
  }

  subscribe(listener: (change: StoreChange) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  get size(): number {
    return this.#events.size;
  }

  get(id: EventId): CalendarEvent | undefined {
    return this.#events.get(id);
  }

  covers(range: DateRange): boolean {
    return rangeCovered(this.#loaded, range);
  }

  /** Cached events overlapping `range`. */
  eventsIn(range: DateRange): CalendarEvent[] {
    const result: CalendarEvent[] = [];
    for (const event of this.#events.values()) {
      if (overlaps(eventBounds(event), range)) result.push(event);
    }
    return result;
  }

  /** Swap the data source (e.g. a different simulated dataset) and drop the cache. */
  reset(source: EventSource): void {
    this.#abort("visible");
    this.#abort("prefetch");
    this.#source = source;
    this.#events.clear();
    this.#confirmed.clear();
    this.#tombstones.clear();
    this.#loaded = [];
    this.loading = false;
    this.loadError = null;
    this.#emit({ type: "reset" });
  }

  /**
   * Ensure `range` is cached. A new visible load cancels any older load;
   * a prefetch only cancels the previous prefetch. Resolves to `false`
   * when the request was superseded.
   */
  async load(range: DateRange, priority: LoadPriority): Promise<boolean> {
    if (this.covers(range)) return true;
    if (priority === "visible") this.#abort("prefetch");
    this.#abort(priority);
    const controller = new AbortController();
    this.#controllers[priority] = controller;
    if (priority === "visible") this.loading = true;
    try {
      const events = await this.#source.queryRange(range, { signal: controller.signal });
      if (controller.signal.aborted) return false;
      this.#merge(range, events);
      if (priority === "visible") this.loadError = null;
      this.#emit({ type: "loaded", range });
      return true;
    } catch (error) {
      if (isAbortError(error)) return false;
      if (priority === "visible") this.loadError = describe(error, "Events could not be loaded.");
      return false;
    } finally {
      if (this.#controllers[priority] === controller) {
        this.#controllers[priority] = null;
        if (priority === "visible") this.loading = false;
      }
    }
  }

  /**
   * Drop cached ranges so the next load fetches fresh data. Used after
   * changes that affect occurrences other than the one edited.
   */
  invalidate(): void {
    this.#abort("visible");
    this.#abort("prefetch");
    this.#loaded = [];
    this.#emit({ type: "invalidated" });
  }

  create(draft: EventDraft): MutationResult {
    const invalid = this.#validate(draft);
    if (invalid) return { ok: false, message: invalid };
    const id = this.#options.newId?.() ?? newEventId();
    const optimistic: CalendarEvent = {
      ...draft,
      id,
      seriesId: null,
      recurrenceId: null,
      rrule: draft.rrule ?? null,
      syncStatus: "pending-local",
      revision: 1,
    };
    this.#set(optimistic);
    this.#source.createEvent(id, draft).then(
      (saved) => {
        this.#settle(id, optimistic.revision, saved);
        // A new series has occurrences beyond the one shown optimistically.
        if (optimistic.rrule) this.invalidate();
      },
      (error: unknown) => {
        if (this.#events.get(id)?.revision === optimistic.revision) this.#delete(id);
        this.#options.onError(describe(error, "The event could not be created."));
      },
    );
    return { ok: true, id };
  }

  /**
   * Edit an event. For an occurrence of a recurring series, `scope` chooses
   * between changing only this occurrence and changing the whole series.
   */
  update(id: EventId, draft: EventDraft, scope: SeriesScope = "occurrence"): MutationResult {
    const previous = this.#events.get(id);
    if (!previous) return { ok: false, message: "The event no longer exists." };
    if (this.#options.isReadOnly(previous.calendarId)) {
      return { ok: false, message: "This event belongs to a read-only calendar." };
    }
    const invalid = this.#validate(draft);
    if (invalid) return { ok: false, message: invalid };
    const effective: EditScope = previous.seriesId ? scope : "event";
    if (effective === "occurrence" && draft.calendarId !== previous.calendarId) {
      return { ok: false, message: "A single occurrence cannot move to another calendar." };
    }
    const rrule = effective === "occurrence" || draft.rrule === undefined ? previous.rrule : draft.rrule;
    const optimistic: CalendarEvent = {
      ...previous,
      ...draft,
      rrule,
      syncStatus: "pending-local",
      revision: previous.revision + 1,
    };
    this.#set(optimistic);
    const affectsOthers = effective === "series" || rrule !== previous.rrule;
    this.#source.updateEvent(id, { ...draft, rrule }, effective).then(
      (saved) => {
        if (affectsOthers) {
          // Other occurrences changed too: let the reload replace this one.
          const current = this.#events.get(id);
          if (current?.revision === optimistic.revision) this.#events.set(id, { ...current, syncStatus: "saved" });
          this.#confirmed.delete(id);
          this.invalidate();
        } else {
          this.#settle(id, optimistic.revision, saved);
        }
      },
      (error: unknown) => {
        // Only roll back if no newer local edit has superseded this one.
        if (this.#events.get(id)?.revision === optimistic.revision) {
          this.#set(this.#confirmed.get(id) ?? previous);
        }
        this.#options.onError(describe(error, "The change could not be saved."));
      },
    );
    return { ok: true, id };
  }

  /**
   * Delete an event. For an occurrence of a series, `scope` chooses between
   * cancelling this occurrence and deleting the whole series.
   */
  remove(id: EventId, scope: SeriesScope = "occurrence"): MutationResult {
    const previous = this.#events.get(id);
    if (!previous) return { ok: false, message: "The event no longer exists." };
    if (this.#options.isReadOnly(previous.calendarId)) {
      return { ok: false, message: "This event belongs to a read-only calendar." };
    }
    const effective: EditScope = previous.seriesId ? scope : "event";
    const removed =
      effective === "series"
        ? [...this.#events.values()].filter((e) => e.seriesId === previous.seriesId)
        : [previous];
    for (const event of removed) {
      this.#tombstones.add(event.id);
      this.#delete(event.id);
    }
    this.#source.deleteEvent(id, effective).then(
      () => {
        for (const event of removed) {
          this.#tombstones.delete(event.id);
          this.#confirmed.delete(event.id);
        }
      },
      (error: unknown) => {
        for (const event of removed) {
          this.#tombstones.delete(event.id);
          this.#set(this.#confirmed.get(event.id) ?? event);
        }
        this.#options.onError(describe(error, "The event could not be deleted."));
      },
    );
    return { ok: true, id };
  }

  #validate(draft: EventDraft): string | null {
    if (this.#options.isReadOnly(draft.calendarId)) return "The selected calendar is read-only.";
    return validateTiming(draft.timing);
  }

  #settle(id: EventId, revision: number, saved: CalendarEvent): void {
    // Keep the local revision counter; the source's numbering is its own.
    const settled: CalendarEvent = { ...saved, revision, syncStatus: "saved" };
    this.#confirmed.set(id, settled);
    if (this.#events.get(id)?.revision === revision) this.#set(settled);
  }

  #merge(range: DateRange, incoming: CalendarEvent[]): void {
    const seen = new Set<EventId>();
    for (const event of incoming) {
      seen.add(event.id);
      if (this.#tombstones.has(event.id)) continue;
      const local = this.#events.get(event.id);
      // A local change still being persisted wins over loaded data.
      if (local?.syncStatus === "pending-local") continue;
      this.#events.set(event.id, event);
      this.#confirmed.set(event.id, event);
    }
    // Drop cached events in this range that the source no longer has.
    for (const [id, event] of this.#events) {
      if (!seen.has(id) && event.syncStatus !== "pending-local" && overlaps(eventBounds(event), range)) {
        this.#events.delete(id);
        this.#confirmed.delete(id);
      }
    }
    this.#loaded = mergeRange(this.#loaded, range);
  }

  #set(event: CalendarEvent): void {
    this.#events.set(event.id, event);
    this.#emit({ type: "upsert", event });
  }

  #delete(id: EventId): void {
    if (this.#events.delete(id)) this.#emit({ type: "remove", id });
  }

  #abort(priority: LoadPriority): void {
    this.#controllers[priority]?.abort();
    this.#controllers[priority] = null;
  }

  #emit(change: StoreChange): void {
    for (const listener of this.#listeners) listener(change);
  }
}

function describe(error: unknown, fallback: string): string {
  return error instanceof SourceError ? error.message : fallback;
}

let fallbackCounter = 0;

export function newEventId(): EventId {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return `local-${crypto.randomUUID()}`;
  }
  return `local-${Date.now().toString(36)}-${(fallbackCounter++).toString(36)}`;
}
