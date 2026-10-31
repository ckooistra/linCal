import { describe, expect, it } from "vitest";
import { buildRule, describeRule, isPreset, parseRule, type RuleForm } from "../services/recurrence";
import { parseIsoDate } from "../services/time";

// Tuesday 16 June 2026, 09:00 local (Europe/Berlin in tests): the third
// Tuesday of June. 30 June is its last Tuesday.
const start = new Date(2026, 5, 16, 9, 0);
const lastTuesday = new Date(2026, 5, 30, 9, 0);

describe("parseRule / buildRule", () => {
  it.each([
    "FREQ=DAILY",
    "FREQ=WEEKLY",
    "FREQ=MONTHLY",
    "FREQ=YEARLY",
    "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
    "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE",
    "FREQ=MONTHLY;BYDAY=3TU",
    "FREQ=DAILY;INTERVAL=3;COUNT=10",
  ])("round-trips %s", (rule) => {
    const form = parseRule(rule, start);
    expect(form).not.toBeNull();
    expect(buildRule(form!, start, "zoned")).toBe(rule);
  });

  it("leaves out what the start implies, so simple rules match the presets", () => {
    expect(buildRule(parseRule("FREQ=WEEKLY;BYDAY=TU", start)!, start, "zoned")).toBe("FREQ=WEEKLY");
    expect(buildRule(parseRule("FREQ=MONTHLY;BYMONTHDAY=16", start)!, start, "zoned")).toBe("FREQ=MONTHLY");
    expect(buildRule(parseRule("FREQ=YEARLY;BYMONTH=6", start)!, start, "zoned")).toBe("FREQ=YEARLY");
    expect(isPreset("FREQ=WEEKLY")).toBe(true);
    expect(isPreset("FREQ=WEEKLY;INTERVAL=2")).toBe(false);
  });

  it("reads the last weekday of the month", () => {
    const form = parseRule("FREQ=MONTHLY;BYDAY=-1TU", lastTuesday)!;
    expect(form.monthly).toBe("last");
    expect(buildRule(form, lastTuesday, "zoned")).toBe("FREQ=MONTHLY;BYDAY=-1TU");
    // The start moved: the choice follows the new start.
    expect(buildRule({ ...form, monthly: "nth" }, start, "zoned")).toBe("FREQ=MONTHLY;BYDAY=3TU");
  });

  it("refuses what the editor cannot show", () => {
    for (const rule of [
      "FREQ=HOURLY",
      "FREQ=MONTHLY;BYMONTHDAY=1,15",
      "FREQ=MONTHLY;BYDAY=2TU", // not the start's position
      "FREQ=MONTHLY;BYDAY=TU",
      "FREQ=WEEKLY;BYSETPOS=1",
      "FREQ=DAILY;COUNT=0",
      "FREQ=DAILY;COUNT=2;UNTIL=20260701",
      "",
      "garbage",
    ]) {
      expect(parseRule(rule, start), rule).toBeNull();
    }
  });

  it("writes UNTIL in the form of the start", () => {
    const form: RuleForm = { ...parseRule("FREQ=DAILY", start)!, end: { kind: "until", date: "2026-07-01" } };
    expect(buildRule(form, start, "date")).toBe("FREQ=DAILY;UNTIL=20260701");
    expect(buildRule(form, start, "floating")).toBe("FREQ=DAILY;UNTIL=20260701T235959");
    // 23:59:59 on 1 July in Berlin (CEST) is 21:59:59 UTC.
    expect(buildRule(form, start, "zoned")).toBe("FREQ=DAILY;UNTIL=20260701T215959Z");
    for (const until of ["20260701", "20260701T235959", "20260701T215959Z"]) {
      expect(parseRule(`FREQ=DAILY;UNTIL=${until}`, start)?.end).toEqual({ kind: "until", date: "2026-07-01" });
    }
  });
});

describe("describeRule", () => {
  it("reads naturally", () => {
    const d = (rule: string, from = start) => describeRule(parseRule(rule, from)!, from);
    expect(d("FREQ=DAILY")).toBe("Every day");
    expect(d("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR;COUNT=6")).toBe(
      "Every 2 weeks on Monday, Wednesday and Friday, 6 times",
    );
    expect(d("FREQ=MONTHLY;BYDAY=3TU")).toBe("Every month on the third Tuesday");
    expect(d("FREQ=MONTHLY;BYDAY=-1TU", lastTuesday)).toBe("Every month on the last Tuesday");
    expect(d("FREQ=MONTHLY;COUNT=1")).toBe("Every month on day 16, once");
    expect(d("FREQ=DAILY;UNTIL=20260701")).toBe(
      `Every day, until ${parseIsoDate("2026-07-01").toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" })}`,
    );
  });
});

describe("reminderLabel", () => {
  it("reads naturally", async () => {
    const { reminderLabel, normalizeReminders } = await import("../services/reminders");
    expect(reminderLabel(0)).toBe("At start");
    expect(reminderLabel(1)).toBe("1 minute before");
    expect(reminderLabel(15)).toBe("15 minutes before");
    expect(reminderLabel(60)).toBe("1 hour before");
    expect(reminderLabel(90)).toBe("1 hour 30 minutes before");
    expect(reminderLabel(1440)).toBe("1 day before");
    expect(reminderLabel(2880 + 60)).toBe("2 days 1 hour before");
    expect(reminderLabel(20160)).toBe("2 weeks before");
    expect(normalizeReminders([60, 5, 60, 0])).toEqual([0, 5, 60]);
  });
});
