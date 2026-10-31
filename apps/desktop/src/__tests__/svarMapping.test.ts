import { describe, expect, it } from "vitest";
import {
  calendarColorCss,
  cssIdent,
  eventClass,
  prefetchWindow,
  textColorFor,
  timingFromWidget,
  toSvarEvent,
} from "../components/calendar/svarMapping";
import { parseIsoDate } from "../services/time";
import type { CalendarEvent } from "../types/calendar";

const timed: CalendarEvent = {
  id: "e1",
  calendarId: "cal-work",
  title: "Review",
  description: "",
  location: "",
  timing: { kind: "timed", start: Date.UTC(2026, 2, 29, 8), end: Date.UTC(2026, 2, 29, 9), timeZone: "Europe/Berlin" },
  seriesId: null,
  syncStatus: "saved",
  revision: 1,
  recurrenceId: null,
  rrule: null,
  reminders: [],
};

describe("svarMapping", () => {
  it("maps timed events and round-trips their timing and zone", () => {
    const s = toSvarEvent(timed, false);
    expect(s).toMatchObject({ id: "e1", text: "Review", allDay: false, calendarId: "cal-work" });
    expect(timingFromWidget(timed.timing, s.start, s.end, false)).toEqual(timed.timing);
  });

  it("maps all-day events to local midnights and back", () => {
    const allDay: CalendarEvent = { ...timed, timing: { kind: "all-day", startDate: "2026-03-28", endDate: "2026-03-30" } };
    const s = toSvarEvent(allDay, false);
    expect(s.start).toEqual(parseIsoDate("2026-03-28"));
    expect(s.end).toEqual(parseIsoDate("2026-03-30"));
    expect(timingFromWidget(allDay.timing, s.start, s.end, true)).toEqual(allDay.timing);
  });

  it("gives all-day timings at least one day", () => {
    const d = parseIsoDate("2026-05-01");
    expect(timingFromWidget(null, d, d, true)).toEqual({ kind: "all-day", startDate: "2026-05-01", endDate: "2026-05-02" });
  });

  it("caches mapped objects per event instance", () => {
    expect(toSvarEvent(timed, false)).toBe(toSvarEvent(timed, false));
    expect(toSvarEvent({ ...timed }, false)).not.toBe(toSvarEvent(timed, false));
  });

  it("builds safe CSS classes and colour rules", () => {
    expect(cssIdent("a b/c")).toBe("a_b_c");
    expect(eventClass({ calendarId: "cal-work", status: "pending-local", readOnly: true })).toBe(
      "uc-ev uc-cal-cal-work uc-st-pending-local uc-ro",
    );
    const css = calendarColorCss([
      { id: "x", accountId: "a", name: "X", color: "#ffff00", readOnly: false },
      { id: "bad", accountId: "a", name: "B", color: "red;}body{", readOnly: false },
    ]);
    expect(css).toContain(".uc-cal-x{--wx-color-primary:#ffff00;--wx-color-primary-font:#000");
    expect(css).not.toContain("bad");
  });

  it("picks readable text colours", () => {
    expect(textColorFor("#000000")).toBe("#fff");
    expect(textColorFor("#ffffff")).toBe("#000");
    expect(textColorFor("#1e3a8a")).toBe("#fff");
    expect(textColorFor("#f59e0b")).toBe("#000");
  });

  it("prefetches one month either side, month-aligned", () => {
    const w = prefetchWindow({ start: parseIsoDate("2026-06-01").getTime(), end: parseIsoDate("2026-07-06").getTime() });
    expect(new Date(w.start)).toEqual(parseIsoDate("2026-05-01"));
    expect(new Date(w.end)).toEqual(parseIsoDate("2026-09-01"));
  });
});

describe("eventLabel", () => {
  it("names the event, its time, calendar and state", async () => {
    const { eventLabel } = await import("../components/calendar/svarMapping");
    const start = new Date(2026, 5, 16, 10, 30);
    const base = {
      id: "e",
      start,
      end: new Date(2026, 5, 16, 11, 15),
      allDay: false,
      text: "Dentist",
      calendarId: "c",
      status: "synced" as const,
      readOnly: false,
    };
    const day = start.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" });
    const t = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    expect(eventLabel(base, "Personal")).toBe(`Dentist, ${day}, ${t(start)} to ${t(base.end)}, Personal`);
    expect(eventLabel({ ...base, readOnly: true, status: "conflict" }, "Team")).toMatch(/, Team, read-only, conflict$/);
    const allDay = { ...base, allDay: true, start: new Date(2026, 5, 16), end: new Date(2026, 5, 17) };
    expect(eventLabel(allDay, "Holidays")).toBe(`Dentist, ${day}, all day, Holidays`);
  });
});
