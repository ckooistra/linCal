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
