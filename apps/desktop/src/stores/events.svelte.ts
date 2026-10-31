import { isAbortError, SourceError, type EventSource, type SearchOptions } from "../services/eventSource";
import { eventBounds, overlaps, validateTiming } from "../services/time";
import type {
  CalendarEvent,
  CalendarId,
  DateRange,
  EditScope,
  EventDraft,
  EventId,
} from "../types/calendar";
import { clipRanges, mergeRange, rangeCovered } from "./ranges";

/** Cached data further than this from the visible range is dropped. */
const RETAIN_MS = 120 * 86_400_000;

export type StoreChange =
  | { type: "upsert"; event: CalendarEvent }
  | { type: "remove"; id: EventId }
  | { type: "loaded"; range: DateRange }
  /** Cached ranges were dropped (e.g. a whole series changed); reload what is visible. */
  | { type: "invalidated" }
  | { type: "reset" };

/** For occurrences of a series: just this one, this and all later ones, or the whole series. */
export type SeriesScope = "occurrence" | "following" | "series";

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
  /**
   * Orders loads against local writes: every write start and completion
   * takes the next number, and a load ignores events written after it
   * started (its data may predate the write).
   */
  #clock = 0;
  readonly #writtenAt = new Map<EventId, number>();
  /** Loads in flight, by the clock value they started at. */
  readonly #inflight = new Set<number>();
  /** Bumped by `reset`: completions of writes to an older source are ignored. */
  #sourceGeneration = 0;
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

  /** Searches all stored events (not just the cache); see `EventSource.search`. */
  search(text: string, range: DateRange, options: SearchOptions): Promise<CalendarEvent[]> {
    return this.#source.search(text, range, options);
  }

  /** Swap the data source (e.g. a different simulated dataset) and drop the cache. */
  reset(source: EventSource): void {
    this.#abort("visible");
    this.#abort("prefetch");
    this.#source = source;
    this.#events.clear();
    this.#confirmed.clear();
    this.#tombstones.clear();
    this.#writtenAt.clear();
    this.#sourceGeneration++;
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
    const startedAt = ++this.#clock;
    this.#inflight.add(startedAt);
    try {
      const events = await this.#source.queryRange(range, { signal: controller.signal });
      if (controller.signal.aborted) return false;
      if (priority === "visible") this.#retainAround(range);
      this.#merge(range, events, startedAt);
      if (priority === "visible") this.loadError = null;
      this.#emit({ type: "loaded", range });
      return true;
    } catch (error) {
      if (isAbortError(error)) return false;
      if (priority === "visible") this.loadError = describe(error, "Events could not be loaded.");
      return false;
    } finally {
      this.#inflight.delete(startedAt);
      this.#forgetOldWrites();
      if (this.#controllers[priority] === controller) {
        this.#controllers[priority] = null;
        if (priority === "visible") this.loading = false;
      }
    }
  }

  /** Marks a local write (start or completion) for load ordering. */
  #wrote(id: EventId): void {
    this.#writtenAt.set(id, ++this.#clock);
  }

  /** Write marks only matter to loads that started before them. */
  #forgetOldWrites(): void {
    const oldest = Math.min(Infinity, ...this.#inflight);
    for (const [id, at] of this.#writtenAt) {
      if (at < oldest) this.#writtenAt.delete(id);
    }
  }

  /**
   * Bounds the cache: drops events and loaded ranges far from `visible`
   * (local changes still being saved are kept).
   */
  #retainAround(visible: DateRange): void {
    const window = { start: visible.start - RETAIN_MS, end: visible.end + RETAIN_MS };
    if (this.#loaded.every((r) => r.start >= window.start && r.end <= window.end)) return;
    this.#loaded = clipRanges(this.#loaded, window);
    for (const [id, event] of this.#events) {
      if (event.syncStatus !== "pending-local" && !overlaps(eventBounds(event), window)) {
        this.#events.delete(id);
        this.#confirmed.delete(id);
        this.#emit({ type: "remove", id });
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
      reminders: draft.reminders ?? [],
      seriesId: null,
      recurrenceId: null,
      rrule: draft.rrule ?? null,
      syncStatus: "pending-local",
      revision: 1,
    };
    this.#set(optimistic);
    this.#wrote(id);
    const generation = this.#sourceGeneration;
    this.#source.createEvent(id, draft).then(
      (saved) => {
        if (generation !== this.#sourceGeneration) return;
        this.#wrote(id);
        this.#settle(id, optimistic.revision, saved);
        // A new series has occurrences beyond the one shown optimistically.
        if (optimistic.rrule) this.invalidate();
      },
      (error: unknown) => {
        if (generation !== this.#sourceGeneration) return;
        this.#wrote(id);
        if (this.#events.get(id)?.revision === optimistic.revision) this.#delete(id);
        this.#options.onError(describe(error, "The event could not be created."));
      },
    );
    return { ok: true, id };
  }

  /**
   * Edit an event. For an occurrence of a recurring series, `scope` chooses
   * between changing only this occurrence, it and the later ones (the
   * series is split), and the whole series.
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
    this.#wrote(id);
    const generation = this.#sourceGeneration;
    const affectsOthers = effective === "series" || effective === "following" || rrule !== previous.rrule;
    const newId = effective === "following" ? (this.#options.newId?.() ?? newEventId()) : undefined;
    this.#source.updateEvent(id, { ...draft, rrule }, effective, newId).then(
      (saved) => {
        if (generation !== this.#sourceGeneration) return;
        this.#wrote(id);
        if (affectsOthers) {
          // Other occurrences changed too: let the reload replace this one.
          const current = this.#events.get(id);
          if (current?.revision === optimistic.revision) {
            this.#events.set(id, { ...current, syncStatus: settledStatus(saved) });
          }
          this.#confirmed.delete(id);
          this.invalidate();
        } else {
          this.#settle(id, optimistic.revision, saved);
        }
      },
      (error: unknown) => {
        if (generation !== this.#sourceGeneration) return;
        this.#wrote(id);
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
   * cancelling this occurrence, ending the series before it, and deleting
   * the whole series.
   */
  remove(id: EventId, scope: SeriesScope = "occurrence"): MutationResult {
    const previous = this.#events.get(id);
    if (!previous) return { ok: false, message: "The event no longer exists." };
    if (this.#options.isReadOnly(previous.calendarId)) {
      return { ok: false, message: "This event belongs to a read-only calendar." };
    }
    const effective: EditScope = previous.seriesId ? scope : "event";
    const from = previous.recurrenceId ?? "";
    const removed =
      effective === "series" || effective === "following"
        ? [...this.#events.values()].filter(
            (e) =>
              e.seriesId === previous.seriesId &&
              // Recurrence ids of one series share a sortable format.
              (effective === "series" || (e.recurrenceId ?? "") >= from),
          )
        : [previous];
    for (const event of removed) {
      this.#tombstones.add(event.id);
      this.#delete(event.id);
      this.#wrote(event.id);
    }
    const generation = this.#sourceGeneration;
    this.#source.deleteEvent(id, effective).then(
      () => {
        if (generation !== this.#sourceGeneration) return;
        for (const event of removed) {
          // Loads that started earlier still skip it (write mark).
          this.#wrote(event.id);
          this.#tombstones.delete(event.id);
          this.#confirmed.delete(event.id);
        }
      },
      (error: unknown) => {
        if (generation !== this.#sourceGeneration) return;
        for (const event of removed) {
          this.#wrote(event.id);
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
    const settled: CalendarEvent = { ...saved, revision, syncStatus: settledStatus(saved) };
    this.#confirmed.set(id, settled);
    if (this.#events.get(id)?.revision === revision) this.#set(settled);
  }

  #merge(range: DateRange, incoming: CalendarEvent[], startedAt: number): void {
    // Written locally after this load started: the loaded copy may be older.
    const newer = (id: EventId) => (this.#writtenAt.get(id) ?? 0) > startedAt;
    const seen = new Set<EventId>();
    for (const event of incoming) {
      seen.add(event.id);
      if (this.#tombstones.has(event.id) || newer(event.id)) continue;
      const local = this.#events.get(event.id);
      // A local change still being persisted wins over loaded data.
      if (local?.syncStatus === "pending-local") continue;
      this.#events.set(event.id, event);
      this.#confirmed.set(event.id, event);
    }
    // Drop cached events in this range that the source no longer has.
    for (const [id, event] of this.#events) {
      if (
        !seen.has(id) &&
        !newer(id) &&
        event.syncStatus !== "pending-local" &&
        overlaps(eventBounds(event), range)
      ) {
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

/** Status after a successful save: the source's (e.g. "pending-sync" for synchronized calendars). */
function settledStatus(saved: CalendarEvent): CalendarEvent["syncStatus"] {
  return saved.syncStatus === "pending-local" ? "saved" : saved.syncStatus;
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
