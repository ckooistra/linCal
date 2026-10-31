import type { EventTiming, EventVersion } from "../types/calendar";
import { describeRule, parseRule } from "./recurrence";
import { reminderLabel } from "./reminders";
import { addIsoDays, parseIsoDate, timingBounds } from "./time";

export interface ComparedField {
  label: string;
  local: string;
  server: string;
  differs: boolean;
}

function when(timing: EventTiming): string {
  if (timing.kind === "all-day") {
    const first = parseIsoDate(timing.startDate);
    const last = addIsoDays(timing.endDate, -1);
    const day = (d: Date) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", year: "numeric" });
    return last === timing.startDate ? `${day(first)}, all day` : `${day(first)} – ${day(parseIsoDate(last))}, all day`;
  }
  const { start, end } = timingBounds(timing);
  const s = new Date(start);
  const e = new Date(end);
  const date = (d: Date) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short", year: "numeric" });
  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  return date(s) === date(e) ? `${date(s)}, ${time(s)} – ${time(e)}` : `${date(s)}, ${time(s)} – ${date(e)}, ${time(e)}`;
}

function repeat(version: EventVersion): string {
  if (version.rrule === null) return "Does not repeat";
  const start = new Date(timingBounds(version.timing).start);
  const form = parseRule(version.rrule, start);
  return form ? describeRule(form, start) : version.rrule || "On specific dates";
}

const MISSING = "(deleted)";

/**
 * The fields of two versions side by side, in display order. A version
 * that is null was deleted on that side.
 */
export function compareVersions(local: EventVersion | null, server: EventVersion | null): ComparedField[] {
  const fields: [string, (v: EventVersion) => string][] = [
    ["Title", (v) => v.title || "(No title)"],
    ["When", (v) => when(v.timing)],
    ["Repeat", repeat],
    ["Reminders", (v) => (v.reminders.length ? v.reminders.map(reminderLabel).join(", ") : "None")],
    ["Location", (v) => v.location || "—"],
    ["Description", (v) => v.description || "—"],
  ];
  return fields.map(([label, show]) => {
    const l = local ? show(local) : MISSING;
    const s = server ? show(server) : MISSING;
    return { label, local: l, server: s, differs: l !== s };
  });
}
