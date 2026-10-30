import { describe, expect, it, vi } from "vitest";
import { generateDataset } from "../services/mock/generator";
import { MockEventSource } from "../services/mock/mockEventSource";
import { parseIsoDate } from "../services/time";
import { EventStore, type StoreChange } from "../stores/events.svelte";
import type { DateRange, EventDraft } from "../types/calendar";

const READ_ONLY = new Set(["cal-holidays", "cal-birthdays"]);

function setup(latencyMs = 0) {
  const data = generateDataset({ seed: 1, eventCount: 500, anchor: "2026-06-15" });
  const source = new MockEventSource(data, { latencyMs });
  const errors: string[] = [];
  const changes: StoreChange[] = [];
  let n = 0;
  const store = new EventStore(source, {
    isReadOnly: (id) => READ_ONLY.has(id),
    onError: (m) => errors.push(m),
    newId: () => `new-${n++}`,
  });
  store.subscribe((c) => changes.push(c));
  return { store, source, errors, changes };
}

const june: DateRange = {
  start: parseIsoDate("2026-06-01").getTime(),
  end: parseIsoDate("2026-07-01").getTime(),
};

const draft: EventDraft = {
  calendarId: "cal-work",
  title: "Planning",
  description: "",
  location: "",
  timing: {
    kind: "timed",
    start: parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000,
    end: parseIsoDate("2026-06-15").getTime() + 10 * 3_600_000,
    timeZone: "UTC",
  },
};

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("EventStore loading", () => {
  it("loads a range once and serves it from cache", async () => {
    const { store, source } = setup();
    const spy = vi.spyOn(source, "queryRange");
    expect(await store.load(june, "visible")).toBe(true);
    expect(store.covers(june)).toBe(true);
    expect(store.eventsIn(june).length).toBeGreaterThan(0);
    await store.load(june, "visible");
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("cancels an obsolete visible load when a newer one starts", async () => {
    const { store } = setup(20);
    const july = { start: june.end, end: parseIsoDate("2026-08-01").getTime() };
    const first = store.load(june, "visible");
    const second = store.load(july, "visible");
    expect(await first).toBe(false);
    expect(await second).toBe(true);
    expect(store.covers(june)).toBe(false);
    expect(store.covers(july)).toBe(true);
    expect(store.loading).toBe(false);
  });

  it("a visible load cancels a pending prefetch", async () => {
    const { store } = setup(20);
    const prefetch = store.load({ start: june.end, end: june.end + 86_400_000 * 30 }, "prefetch");
    const visible = store.load(june, "visible");
    expect(await prefetch).toBe(false);
    expect(await visible).toBe(true);
  });
});

describe("EventStore optimistic writes", () => {
  it("shows a created event immediately, then marks it saved", async () => {
    const { store, changes } = setup(10);
    const result = store.create(draft);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(store.get(result.id)?.syncStatus).toBe("pending-local");
    expect(changes.at(-1)).toMatchObject({ type: "upsert" });
    await new Promise((r) => setTimeout(r, 30));
    expect(store.get(result.id)?.syncStatus).toBe("saved");
  });

  it("emits one incremental change per edit instead of a reload", async () => {
    const { store, changes } = setup();
    await store.load(june, "visible");
    const target = store.eventsIn(june).find((e) => !READ_ONLY.has(e.calendarId))!;
    changes.length = 0;
    store.update(target.id, { ...draft, calendarId: target.calendarId, title: "Moved" });
    expect(changes).toEqual([{ type: "upsert", event: expect.objectContaining({ id: target.id, title: "Moved" }) }]);
    await flush();
    expect(store.get(target.id)?.syncStatus).toBe("saved");
  });

  it("rolls back a failed update and reports it", async () => {
    const { store, source, errors } = setup();
    await store.load(june, "visible");
    const target = store.eventsIn(june).find((e) => !READ_ONLY.has(e.calendarId))!;
    source.failNextWrites = 1;
    store.update(target.id, { ...draft, calendarId: target.calendarId, title: "Doomed" });
    expect(store.get(target.id)?.title).toBe("Doomed");
    await flush();
    expect(store.get(target.id)?.title).toBe(target.title);
    expect(errors).toEqual(["Simulated storage failure."]);
  });

  it("does not let a stale failure undo a newer edit", async () => {
    const { store, source } = setup(5);
    await store.load(june, "visible");
    const target = store.eventsIn(june).find((e) => !READ_ONLY.has(e.calendarId))!;
    source.failNextWrites = 1;
    store.update(target.id, { ...draft, calendarId: target.calendarId, title: "First" });
    store.update(target.id, { ...draft, calendarId: target.calendarId, title: "Second" });
    await new Promise((r) => setTimeout(r, 30));
    expect(store.get(target.id)?.title).toBe("Second");
    expect(store.get(target.id)?.syncStatus).toBe("saved");
  });

  it("removes a failed creation", async () => {
    const { store, source, errors } = setup();
    source.failNextWrites = 1;
    const result = store.create(draft);
    if (!result.ok) throw new Error("expected ok");
    await flush();
    expect(store.get(result.id)).toBeUndefined();
    expect(errors).toHaveLength(1);
  });

  it("restores a deleted event when deletion fails", async () => {
    const { store, source } = setup();
    await store.load(june, "visible");
    const target = store.eventsIn(june).find((e) => !READ_ONLY.has(e.calendarId))!;
    source.failNextWrites = 1;
    store.remove(target.id);
    expect(store.get(target.id)).toBeUndefined();
    await flush();
    expect(store.get(target.id)?.id).toBe(target.id);
  });

  it("refuses edits on read-only calendars without touching state", async () => {
    const { store, changes } = setup();
    await store.load(june, "visible");
    const holiday = store.eventsIn({ start: 0, end: Number.MAX_SAFE_INTEGER }).find((e) => READ_ONLY.has(e.calendarId));
    changes.length = 0;
    if (holiday) {
      expect(store.update(holiday.id, { ...draft, calendarId: holiday.calendarId }).ok).toBe(false);
      expect(store.remove(holiday.id).ok).toBe(false);
    }
    expect(store.create({ ...draft, calendarId: "cal-holidays" }).ok).toBe(false);
    expect(changes).toEqual([]);
  });

  it("rejects invalid timings", () => {
    const { store } = setup();
    const result = store.create({ ...draft, timing: { ...draft.timing, end: 0 } as EventDraft["timing"] });
    expect(result).toEqual({ ok: false, message: "The event must end after it starts." });
  });

  it("does not overwrite a pending local edit with loaded data", async () => {
    const { store } = setup(10);
    const result = store.create(draft);
    if (!result.ok) throw new Error("expected ok");
    await store.load(june, "visible");
    expect(store.get(result.id)).toBeDefined();
  });
});

describe("EventStore with recurring occurrences", () => {
  function seriesSetup() {
    const ctx = setup();
    const start = parseIsoDate("2026-06-15").getTime() + 9 * 3_600_000;
    const occ = (day: number) => ({
      id: `s::${day}`,
      calendarId: "cal-team",
      title: "Stand-up",
      description: "",
      location: "",
      timing: { kind: "timed" as const, start: start + day * 86_400_000, end: start + day * 86_400_000 + 900_000, timeZone: "UTC" },
      seriesId: "s",
      recurrenceId: String(day),
      rrule: "FREQ=DAILY",
      syncStatus: "saved" as const,
      revision: 1,
    });
    return { ...ctx, occ };
  }

  it("passes the scope to the source and reloads after series edits", async () => {
    const { store, source, changes, occ } = seriesSetup();
    // Seed two occurrences through the source so the store can load them.
    const created = [occ(0), occ(1)];
    vi.spyOn(source, "queryRange").mockResolvedValue(created);
    await store.load(june, "visible");
    const update = vi.spyOn(source, "updateEvent").mockResolvedValue({ ...occ(0), title: "Daily sync" });

    store.update("s::0", { ...occ(0), title: "Moved" }, "occurrence");
    await flush();
    expect(update).toHaveBeenLastCalledWith("s::0", expect.objectContaining({ title: "Moved" }), "occurrence");
    expect(changes.some((c) => c.type === "invalidated")).toBe(false);

    store.update("s::1", { ...occ(1), title: "Daily sync" }, "series");
    await flush();
    expect(update).toHaveBeenLastCalledWith("s::1", expect.objectContaining({ title: "Daily sync" }), "series");
    expect(changes.at(-1)).toEqual({ type: "invalidated" });
    expect(store.covers(june)).toBe(false);
  });

  it("refuses to move a single occurrence to another calendar", async () => {
    const { store, source, occ } = seriesSetup();
    vi.spyOn(source, "queryRange").mockResolvedValue([occ(0)]);
    await store.load(june, "visible");
    expect(store.update("s::0", { ...occ(0), calendarId: "cal-work" }, "occurrence").ok).toBe(false);
  });

  it("removes every cached occurrence when deleting a series, and restores them on failure", async () => {
    const { store, source, errors, occ } = seriesSetup();
    vi.spyOn(source, "queryRange").mockResolvedValue([occ(0), occ(1), occ(2)]);
    await store.load(june, "visible");
    const del = vi.spyOn(source, "deleteEvent").mockRejectedValueOnce(new Error("disk full"));
    store.remove("s::1", "series");
    expect(store.get("s::0")).toBeUndefined();
    expect(store.get("s::2")).toBeUndefined();
    expect(del).toHaveBeenCalledWith("s::1", "series");
    await flush();
    expect(store.get("s::0")?.id).toBe("s::0");
    expect(store.get("s::2")?.id).toBe("s::2");
    expect(errors).toEqual(["The event could not be deleted."]);
  });

  it("cancels a single occurrence without touching the others", async () => {
    const { store, source, occ } = seriesSetup();
    vi.spyOn(source, "queryRange").mockResolvedValue([occ(0), occ(1)]);
    await store.load(june, "visible");
    const del = vi.spyOn(source, "deleteEvent").mockResolvedValue();
    store.remove("s::0", "occurrence");
    expect(del).toHaveBeenCalledWith("s::0", "occurrence");
    expect(store.get("s::1")).toBeDefined();
  });
});

describe("EventStore ordering (audit F13, F14, F20)", () => {
  it("does not let an older load overwrite a completed edit", async () => {
    const { store, source } = setup();
    await store.load(june, "visible");
    const original = store.eventsIn(june).find((e) => e.calendarId === "cal-work" && !e.seriesId)!;
    store.invalidate();
    // Hold a reload that captured the data before the edit.
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const query = source.queryRange.bind(source);
    vi.spyOn(source, "queryRange").mockImplementationOnce(async (range, opts) => {
      const snapshot = await query(range, opts);
      await gate;
      return snapshot;
    });
    const reload = store.load(june, "visible");
    store.update(original.id, { ...original, title: "Renamed" });
    await flush();
    expect(store.get(original.id)?.syncStatus).toBe("saved");
    release();
    await reload;
    expect(store.get(original.id)?.title).toBe("Renamed");
  });

  it("keeps the sync status the source reports", async () => {
    const { store, source } = setup();
    vi.spyOn(source, "createEvent").mockImplementationOnce(async (id, d) => ({
      ...d,
      id,
      seriesId: null,
      recurrenceId: null,
      rrule: null,
      syncStatus: "pending-sync",
      revision: 1,
    }));
    const result = store.create(draft);
    if (!result.ok) throw new Error("expected ok");
    await flush();
    expect(store.get(result.id)?.syncStatus).toBe("pending-sync");
  });

  it("drops cached data far from what is visible", async () => {
    const { store } = setup();
    await store.load(june, "visible");
    expect(store.eventsIn(june).length).toBeGreaterThan(0);
    const far = { start: parseIsoDate("2027-06-01").getTime(), end: parseIsoDate("2027-07-01").getTime() };
    await store.load(far, "visible");
    expect(store.covers(june)).toBe(false);
    expect(store.eventsIn(june)).toEqual([]);
  });
});
