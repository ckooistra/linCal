import { describe, expect, it } from "vitest";
import { DATASET_SIZES, generateDataset } from "../services/mock/generator";
import { eventBounds, overlaps, parseIsoDate } from "../services/time";

const base = { seed: 42, anchor: "2026-06-15", timeZone: "Europe/Berlin" };

describe("generateDataset", () => {
  it("is deterministic for the same options", () => {
    const a = generateDataset({ ...base, eventCount: 1000 });
    const b = generateDataset({ ...base, eventCount: 1000 });
    expect(a).toEqual(b);
  });

  it("differs for different seeds", () => {
    const a = generateDataset({ ...base, eventCount: 200 });
    const b = generateDataset({ ...base, seed: 43, eventCount: 200 });
    expect(a.events).not.toEqual(b.events);
  });

  it.each(DATASET_SIZES)("produces exactly %i events with unique ids", (size) => {
    const { events } = generateDataset({ ...base, eventCount: size });
    expect(events).toHaveLength(size);
    expect(new Set(events.map((e) => e.id)).size).toBe(size);
  });

  it("covers multiple calendars, all-day, recurring and overlapping events", () => {
    const { events, calendars } = generateDataset({ ...base, eventCount: 1000 });
    const used = new Set(events.map((e) => e.calendarId));
    expect(used.size).toBeGreaterThanOrEqual(6);
    expect(calendars.some((c) => c.readOnly)).toBe(true);
    expect(events.some((e) => e.timing.kind === "all-day")).toBe(true);
    expect(events.some((e) => e.seriesId !== null)).toBe(true);
    // At least one day has several mutually overlapping timed events.
    const timed = events.filter((e) => e.timing.kind === "timed").map(eventBounds);
    const maxOverlap = Math.max(...timed.map((r) => timed.filter((o) => overlaps(o, r)).length));
    expect(maxOverlap).toBeGreaterThanOrEqual(4);
  });

  it("keeps every event valid and within about six months of the anchor", () => {
    const { events } = generateDataset({ ...base, eventCount: 5000 });
    const anchor = parseIsoDate(base.anchor).getTime();
    const limit = 190 * 86_400_000;
    for (const e of events) {
      const { start, end } = eventBounds(e);
      expect(end).toBeGreaterThan(start);
      expect(Math.abs(start - anchor)).toBeLessThan(limit);
    }
  });

  it("keeps recurring occurrences at the same wall-clock time across DST", () => {
    // Europe/Berlin switches to winter time on 2026-10-25.
    const { events } = generateDataset({ ...base, eventCount: 5000, anchor: "2026-10-25" });
    const standups = events.filter((e) => e.seriesId === "series-standup");
    expect(standups.length).toBeGreaterThan(10);
    for (const e of standups) {
      if (e.timing.kind !== "timed") throw new Error("expected timed");
      const start = new Date(e.timing.start);
      expect([start.getHours(), start.getMinutes()]).toEqual([9, 30]);
    }
  });

  it("rejects invalid counts", () => {
    expect(() => generateDataset({ ...base, eventCount: -1 })).toThrow(RangeError);
  });
});
