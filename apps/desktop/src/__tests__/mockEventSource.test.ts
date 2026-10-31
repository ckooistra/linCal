import { describe, expect, it } from "vitest";
import { generateDataset } from "../services/mock/generator";
import { MockEventSource } from "../services/mock/mockEventSource";
import { isAbortError, SourceError } from "../services/eventSource";
import { eventBounds, overlaps, parseIsoDate } from "../services/time";
import type { EventDraft } from "../types/calendar";

const dataset = () => generateDataset({ seed: 7, eventCount: 2000, anchor: "2026-06-15" });

const draft: EventDraft = {
  calendarId: "cal-work",
  title: "New",
  description: "",
  location: "",
  timing: { kind: "timed", start: Date.UTC(2026, 5, 15, 8), end: Date.UTC(2026, 5, 15, 9), timeZone: "UTC" },
};

describe("MockEventSource", () => {
  it("returns exactly the events overlapping a range (matches a linear scan)", async () => {
    const data = dataset();
    const source = new MockEventSource(data);
    const ranges = [
      { start: parseIsoDate("2026-06-01").getTime(), end: parseIsoDate("2026-07-01").getTime() },
      { start: parseIsoDate("2026-06-15").getTime(), end: parseIsoDate("2026-06-16").getTime() },
      { start: parseIsoDate("2030-01-01").getTime(), end: parseIsoDate("2030-02-01").getTime() },
    ];
    for (const range of ranges) {
      const expected = data.events.filter((e) => overlaps(eventBounds(e), range)).map((e) => e.id).sort();
      const actual = (await source.queryRange(range)).map((e) => e.id).sort();
      expect(actual).toEqual(expected);
    }
  });

  it("includes long events that start before the range", async () => {
    const source = new MockEventSource({ ...dataset(), events: [] });
    await source.createEvent("long", {
      ...draft,
      timing: { kind: "all-day", startDate: "2026-05-25", endDate: "2026-06-10" },
    });
    const result = await source.queryRange({
      start: parseIsoDate("2026-06-05").getTime(),
      end: parseIsoDate("2026-06-06").getTime(),
    });
    expect(result.map((e) => e.id)).toEqual(["long"]);
  });

  it("supports create, update and delete", async () => {
    const source = new MockEventSource({ ...dataset(), events: [] });
    const created = await source.createEvent("a", draft);
    expect(created.syncStatus).toBe("saved");
    const updated = await source.updateEvent("a", { ...draft, title: "Changed" });
    expect(updated.title).toBe("Changed");
    await source.deleteEvent("a");
    expect(source.size).toBe(0);
  });

  it("searches every word in title, location and description, with limits", async () => {
    const source = new MockEventSource({ ...dataset(), events: [] });
    const day = (d: number) => ({
      ...draft,
      timing: { ...draft.timing, start: Date.UTC(2026, 5, d, 8), end: Date.UTC(2026, 5, d, 9) } as const,
    });
    await source.createEvent("a", { ...day(1), title: "Team review", location: "Room Ü" });
    await source.createEvent("b", { ...day(2), title: "Review", description: "quarterly team" });
    await source.createEvent("c", { ...day(3), title: "Lunch" });
    const june = { start: Date.UTC(2026, 5, 1), end: Date.UTC(2026, 6, 1) };
    const ids = async (text: string, limit = 10, latest = false) =>
      (await source.search(text, june, { limit, latest })).map((e) => e.id);
    expect(await ids("REVIEW team")).toEqual(["a", "b"]);
    expect(await ids("room ü")).toEqual(["a"]);
    expect(await ids("review lunch")).toEqual([]);
    expect(await ids("  ")).toEqual([]);
    expect(await ids("review", 1)).toEqual(["a"]);
    expect(await ids("review", 1, true)).toEqual(["b"]);
  });

  it("rejects writes to read-only calendars", async () => {
    const source = new MockEventSource(dataset());
    await expect(source.createEvent("x", { ...draft, calendarId: "cal-holidays" })).rejects.toBeInstanceOf(
      SourceError,
    );
  });

  it("injects failures on demand", async () => {
    const source = new MockEventSource(dataset());
    source.failNextWrites = 1;
    await expect(source.createEvent("x", draft)).rejects.toThrow("Simulated");
    await expect(source.createEvent("x", draft)).resolves.toBeTruthy();
  });

  it("aborts delayed queries", async () => {
    const source = new MockEventSource(dataset(), { latencyMs: 50 });
    const controller = new AbortController();
    const promise = source.queryRange({ start: 0, end: 1 }, { signal: controller.signal });
    controller.abort();
    await expect(promise).rejects.toSatisfy(isAbortError);
  });
});
