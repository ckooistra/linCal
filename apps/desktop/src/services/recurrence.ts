import type { IsoDate } from "../types/calendar";
import { addDays, parseIsoDate, toIsoDate } from "./time";

/** Repeat options offered by the editor. Rules are evaluated by the Rust core. */
export interface RepeatPreset {
  label: string;
  rrule: string | null;
}

export const REPEAT_PRESETS: readonly RepeatPreset[] = [
  { label: "Does not repeat", rrule: null },
  { label: "Daily", rrule: "FREQ=DAILY" },
  { label: "Every weekday (Mon–Fri)", rrule: "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR" },
  { label: "Weekly", rrule: "FREQ=WEEKLY" },
  { label: "Monthly", rrule: "FREQ=MONTHLY" },
  { label: "Yearly", rrule: "FREQ=YEARLY" },
];

export function isPreset(rrule: string | null): boolean {
  return REPEAT_PRESETS.some((p) => p.rrule === rrule);
}

export type Frequency = "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY";
export type Weekday = "MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU";

/** Monday first; index = (JS getDay() + 6) % 7. */
export const WEEKDAYS: readonly Weekday[] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

export type RuleEnd = { kind: "never" } | { kind: "until"; date: IsoDate } | { kind: "count"; count: number };

/**
 * The rules the custom repeat editor can show and build. Anything else
 * (BYSETPOS, several month days, hourly, …) is kept as written but not
 * editable.
 */
export interface RuleForm {
  frequency: Frequency;
  interval: number;
  /** Weekly: the days; empty means the start's weekday. */
  weekdays: Weekday[];
  /** Monthly: on the start's day of the month, its nth weekday, or its last weekday. */
  monthly: "day" | "nth" | "last";
  end: RuleEnd;
}

export function weekdayOf(date: Date): Weekday {
  return WEEKDAYS[(date.getDay() + 6) % 7]!;
}

/** Which occurrence of its weekday in the month `date` is (1–5). */
export function nthInMonth(date: Date): number {
  return Math.ceil(date.getDate() / 7);
}

/** Whether `date` is the last of its weekday in its month. */
export function isLastInMonth(date: Date): boolean {
  return addDays(date, 7).getMonth() !== date.getMonth();
}

export function defaultForm(start: Date): RuleForm {
  return { frequency: "WEEKLY", interval: 1, weekdays: [weekdayOf(start)], monthly: "day", end: { kind: "never" } };
}

/**
 * Reads `rrule` into the form, or null when the editor cannot represent
 * it. `start` is the series start, which fills in what the rule leaves out.
 */
export function parseRule(rrule: string | null, start: Date): RuleForm | null {
  if (!rrule) return null;
  const parts = new Map<string, string>();
  for (const part of rrule.split(";")) {
    if (!part.trim()) continue;
    const eq = part.indexOf("=");
    if (eq < 1) return null;
    parts.set(part.slice(0, eq).trim().toUpperCase(), part.slice(eq + 1).trim().toUpperCase());
  }
  const frequency = parts.get("FREQ");
  if (frequency !== "DAILY" && frequency !== "WEEKLY" && frequency !== "MONTHLY" && frequency !== "YEARLY") return null;
  const form: RuleForm = { ...defaultForm(start), frequency };
  for (const [name, value] of parts) {
    switch (name) {
      case "FREQ":
      case "WKST":
        break;
      case "INTERVAL": {
        const n = Number(value);
        if (!Number.isInteger(n) || n < 1) return null;
        form.interval = n;
        break;
      }
      case "COUNT": {
        const n = Number(value);
        if (!Number.isInteger(n) || n < 1) return null;
        form.end = { kind: "count", count: n };
        break;
      }
      case "UNTIL": {
        const date = untilDate(value);
        if (!date) return null;
        form.end = { kind: "until", date };
        break;
      }
      case "BYDAY": {
        const days = value.split(",");
        if (frequency === "WEEKLY" && days.every((d) => (WEEKDAYS as readonly string[]).includes(d))) {
          form.weekdays = WEEKDAYS.filter((d) => days.includes(d));
        } else if (frequency === "MONTHLY" && days.length === 1) {
          const m = /^([+-]?\d)([A-Z]{2})$/.exec(days[0]!);
          if (!m || m[2] !== weekdayOf(start)) return null;
          const n = Number(m[1]);
          if (n === -1 && isLastInMonth(start)) form.monthly = "last";
          else if (n === nthInMonth(start)) form.monthly = "nth";
          else return null;
        } else {
          return null;
        }
        break;
      }
      case "BYMONTHDAY":
        if (frequency !== "MONTHLY" || Number(value) !== start.getDate()) return null;
        break;
      case "BYMONTH":
        if (frequency !== "YEARLY" || Number(value) !== start.getMonth() + 1) return null;
        break;
      default:
        return null;
    }
  }
  if (parts.has("COUNT") && parts.has("UNTIL")) return null;
  return form;
}

/** The local date of an UNTIL value (date, local date-time or UTC). */
function untilDate(value: string): IsoDate | null {
  const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})(Z?))?$/.exec(value);
  if (!m) return null;
  const [, y, mo, d, h, mi, s, z] = m;
  if (h === undefined) return `${y}-${mo}-${d}`;
  const date = z
    ? new Date(Date.UTC(+y!, +mo! - 1, +d!, +h, +mi!, +s!))
    : new Date(+y!, +mo! - 1, +d!, +h, +mi!, +s!);
  return toIsoDate(date);
}

/** How the series' start is expressed, which UNTIL has to match (RFC 5545). */
export type StartForm = "date" | "floating" | "zoned";

/**
 * The RRULE for a form. What the start already implies is left out, so
 * simple rules come out as the presets (e.g. `FREQ=WEEKLY`).
 */
export function buildRule(form: RuleForm, start: Date, startForm: StartForm): string {
  const parts = [`FREQ=${form.frequency}`];
  if (form.interval > 1) parts.push(`INTERVAL=${Math.floor(form.interval)}`);
  if (form.frequency === "WEEKLY") {
    const days = WEEKDAYS.filter((d) => form.weekdays.includes(d));
    if (days.length > 0 && !(days.length === 1 && days[0] === weekdayOf(start))) {
      parts.push(`BYDAY=${days.join(",")}`);
    }
  }
  if (form.frequency === "MONTHLY" && form.monthly !== "day") {
    const n = form.monthly === "last" ? -1 : nthInMonth(start);
    parts.push(`BYDAY=${n}${weekdayOf(start)}`);
  }
  if (form.end.kind === "count") parts.push(`COUNT=${Math.max(1, Math.floor(form.end.count))}`);
  if (form.end.kind === "until") parts.push(`UNTIL=${untilValue(form.end.date, startForm)}`);
  return parts.join(";");
}

/** UNTIL for the last day `date` (inclusive), in the form of the start. */
function untilValue(date: IsoDate, startForm: StartForm): string {
  const compact = date.replaceAll("-", "");
  if (startForm === "date") return compact;
  if (startForm === "floating") return `${compact}T235959`;
  // A UTC instant: the end of that day here.
  const end = parseIsoDate(date);
  end.setHours(23, 59, 59, 0);
  return end.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}/, "");
}

const DAY_NAMES: Record<Weekday, string> = {
  MO: "Monday",
  TU: "Tuesday",
  WE: "Wednesday",
  TH: "Thursday",
  FR: "Friday",
  SA: "Saturday",
  SU: "Sunday",
};
const ORDINALS = ["first", "second", "third", "fourth", "fifth"];

export function weekdayName(day: Weekday): string {
  return DAY_NAMES[day];
}

/** Monthly choice labels for a start date, e.g. "on the third Tuesday". */
export function monthlyLabel(choice: RuleForm["monthly"], start: Date): string {
  const day = DAY_NAMES[weekdayOf(start)];
  if (choice === "day") return `on day ${start.getDate()}`;
  if (choice === "last") return `on the last ${day}`;
  return `on the ${ORDINALS[nthInMonth(start) - 1]} ${day}`;
}

/** A sentence for a form, e.g. "Every 2 weeks on Monday and Wednesday, 10 times". */
export function describeRule(form: RuleForm, start: Date): string {
  const unit = { DAILY: "day", WEEKLY: "week", MONTHLY: "month", YEARLY: "year" }[form.frequency];
  let text = form.interval === 1 ? `Every ${unit}` : `Every ${form.interval} ${unit}s`;
  if (form.frequency === "WEEKLY") {
    const days = WEEKDAYS.filter((d) => form.weekdays.includes(d)).map((d) => DAY_NAMES[d]);
    if (days.length > 0) text += ` on ${days.length === 1 ? days[0] : `${days.slice(0, -1).join(", ")} and ${days.at(-1)}`}`;
  } else if (form.frequency === "MONTHLY") {
    text += ` ${monthlyLabel(form.monthly, start)}`;
  } else if (form.frequency === "YEARLY") {
    text += ` on ${start.toLocaleDateString(undefined, { day: "numeric", month: "long" })}`;
  }
  if (form.end.kind === "count") text += form.end.count === 1 ? ", once" : `, ${form.end.count} times`;
  if (form.end.kind === "until") {
    text += `, until ${parseIsoDate(form.end.date).toLocaleDateString(undefined, { day: "numeric", month: "long", year: "numeric" })}`;
  }
  return text;
}
