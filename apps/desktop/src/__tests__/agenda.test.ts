import { describe, expect, it } from "vitest";
import { agendaDays } from "../services/agenda";
import { parseIsoDate } from "../services/time";
import type { CalendarEvent, EventTiming } from "../types/calendar";

function event(id: string, timing: EventTiming, title = id): CalendarEvent {
  return {
    id,
    calendarId: "cal",
    title,
    description: "",
    location: "",
    timing,
    seriesId: null,
    recurrenceId: null,
    rrule: null,
    syncStatus: "saved",
    revision: 1,
  };
}

/** Local time on a June 2026 day. */
const at = (day: number, hours: number, minutes = 0) =>
  parseIsoDate(`2026-06-${String(day).padStart(2, "0")}`).getTime() + (hours * 60 + minutes) * 60_000;
const timed = (start: number, end: number): EventTiming => ({ kind: "timed", start, end, timeZone: null });
const june = { start: parseIsoDate("2026-06-01").getTime(), end: parseIsoDate("2026-07-01").getTime() };
const summary = (events: CalendarEvent[], spread = true, range = june) =>
  agendaDays(events, range, { spread, hour12: false }).map((d) => [
    d.date,
    d.items.map((i) => `${i.event.id} ${i.when}`),
  ]);

describe("agendaDays", () => {
  it("groups by local day, all-day first, then by start", () => {
    const events = [
      event("late", timed(at(2, 15), at(2, 16))),
      event("early", timed(at(2, 9), at(2, 9, 30))),
      event("holiday", { kind: "all-day", startDate: "2026-06-02", endDate: "2026-06-03" }),
      event("other", timed(at(1, 12), at(1, 13))),
    ];
    expect(summary(events)).toEqual([
      ["2026-06-01", ["other 12:00 – 13:00"]],
      ["2026-06-02", ["holiday All day", "early 09:00 – 09:30", "late 15:00 – 16:00"]],
    ]);
  });

  it("spreads multi-day events over their days, or lists them once", () => {
    const trip = event("trip", timed(at(3, 18), at(5, 10)));
    const camp = event("camp", { kind: "all-day", startDate: "2026-06-29", endDate: "2026-07-03" });
    expect(summary([trip, camp])).toEqual([
      ["2026-06-03", ["trip From 18:00"]],
      ["2026-06-04", ["trip All day"]],
      ["2026-06-05", ["trip Until 10:00"]],
      ["2026-06-29", ["camp All day"]],
      ["2026-06-30", ["camp All day"]],
    ]);
    expect(summary([trip], false)).toEqual([["2026-06-03", ["trip From 18:00"]]]);
  });

  it("clips to the range, keeps events ending at midnight on one day, and shows instants", () => {
    const night = event("night", timed(at(10, 22), at(11, 0)));
    const ping = event("ping", timed(at(12, 8), at(12, 8)));
    const before = event("before", timed(parseIsoDate("2026-05-31").getTime() + 20 * 3_600_000, at(1, 2)));
    expect(summary([night, ping, before])).toEqual([
      ["2026-06-01", ["before Until 02:00"]],
      ["2026-06-10", ["night 22:00 – 00:00"]],
      ["2026-06-12", ["ping 08:00"]],
    ]);
  });
});
