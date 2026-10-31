import { describe, expect, it } from "vitest";
import { compareVersions } from "../services/conflict";
import { parseIsoDate } from "../services/time";
import type { EventVersion } from "../types/calendar";

const start = parseIsoDate("2026-06-16").getTime() + 9 * 3_600_000;
const version: EventVersion = {
  title: "Stand-up",
  description: "",
  location: "Room 4",
  timing: { kind: "timed", start, end: start + 900_000, timeZone: "Europe/Berlin" },
  rrule: "FREQ=WEEKLY;BYDAY=TU,TH",
  reminders: [10],
};

describe("compareVersions", () => {
  it("lists every field and marks the differing ones", () => {
    const rows = compareVersions(version, { ...version, title: "Daily", reminders: [] });
    expect(rows.map((r) => r.label)).toEqual(["Title", "When", "Repeat", "Reminders", "Location", "Description"]);
    expect(rows.filter((r) => r.differs).map((r) => [r.label, r.local, r.server])).toEqual([
      ["Title", "Stand-up", "Daily"],
      ["Reminders", "10 minutes before", "None"],
    ]);
    expect(rows.find((r) => r.label === "Repeat")?.local).toBe("Every week on Tuesday and Thursday");
  });

  it("shows a version deleted on one side", () => {
    const rows = compareVersions(version, null);
    expect(rows.every((r) => r.server === "(deleted)" && r.differs)).toBe(true);
  });

  it("shows when the time differs", () => {
    const later = { ...version, timing: { ...version.timing, start: start + 3_600_000, end: start + 4_500_000 } } as EventVersion;
    const when = compareVersions(version, later).find((r) => r.label === "When")!;
    expect(when.differs).toBe(true);
    expect(when.local).toContain("09:00");
    expect(when.server).toContain("10:00");
  });
});
