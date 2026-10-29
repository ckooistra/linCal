import { describe, expect, it } from "vitest";
import {
  addDays,
  addIsoDays,
  combineDateTime,
  formatDuration,
  formatTimeLabel,
  parseIsoDate,
  parseTimeInput,
  toIsoDate,
  validateTiming,
} from "../services/time";

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

describe("time-of-day input", () => {
  it.each([
    ["9", "09:00"],
    ["930", "09:30"],
    ["0930", "09:30"],
    ["9:30", "09:30"],
    ["9.30", "09:30"],
    ["21h30", "21:30"],
    ["21:30", "21:30"],
    ["9:30pm", "21:30"],
    ["9:30 PM", "21:30"],
    ["9 a", "09:00"],
    ["12am", "00:00"],
    ["12:15 pm", "12:15"],
    [" 7:05 ", "07:05"],
  ])("parses %s as %s", (text, expected) => {
    expect(parseTimeInput(text)).toBe(expected);
  });

  it.each(["", "24:00", "9:60", "13pm", "0am", "abc", "9:3", "12345"])("rejects %s", (text) => {
    expect(parseTimeInput(text)).toBeNull();
  });

  it("formats labels for 12- and 24-hour clocks", () => {
    expect(formatTimeLabel("00:00", true)).toBe("12:00 AM");
    expect(formatTimeLabel("12:30", true)).toBe("12:30 PM");
    expect(formatTimeLabel("21:05", true)).toBe("9:05 PM");
    expect(formatTimeLabel("21:05", false)).toBe("21:05");
    expect(formatDuration(30)).toBe("30 min");
    expect(formatDuration(60)).toBe("1 h");
    expect(formatDuration(90)).toBe("1 h 30 min");
  });
});
