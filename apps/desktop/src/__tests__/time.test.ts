import { describe, expect, it } from "vitest";
import { addDays, addIsoDays, combineDateTime, parseIsoDate, toIsoDate, validateTiming } from "../services/time";

// The test script runs with TZ=Europe/Berlin, which has DST transitions.
describe("time helpers", () => {
  it("runs in a DST zone", () => {
    expect(Intl.DateTimeFormat().resolvedOptions().timeZone).toBe("Europe/Berlin");
  });

  it("parses and formats ISO dates, rejecting impossible ones", () => {
    expect(toIsoDate(parseIsoDate("2026-02-28"))).toBe("2026-02-28");
    expect(() => parseIsoDate("2026-02-30")).toThrow(RangeError);
    expect(() => parseIsoDate("26-2-3")).toThrow(RangeError);
  });

  it("adds calendar days across DST without drifting wall-clock time", () => {
    // 2026-03-29 is a 23-hour day in Berlin.
    const before = combineDateTime("2026-03-28", "09:30")!;
    const after = addDays(before, 2);
    expect([after.getHours(), after.getMinutes()]).toEqual([9, 30]);
    expect(after.getTime() - before.getTime()).toBe(47 * 3_600_000);
    expect(addIsoDays("2026-10-24", 2)).toBe("2026-10-26");
  });

  it("combines dates and times and rejects malformed input", () => {
    expect(combineDateTime("2026-06-01", "7:05")?.getHours()).toBe(7);
    expect(combineDateTime("2026-06-01", "24:00")).toBeNull();
    expect(combineDateTime("nope", "10:00")).toBeNull();
  });

  it("validates timings", () => {
    expect(validateTiming({ kind: "all-day", startDate: "2026-01-02", endDate: "2026-01-02" })).not.toBeNull();
    expect(validateTiming({ kind: "all-day", startDate: "2026-01-02", endDate: "2026-01-03" })).toBeNull();
    expect(validateTiming({ kind: "timed", start: 10, end: 5, timeZone: null })).not.toBeNull();
  });
});
