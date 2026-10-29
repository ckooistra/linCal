import type { CalendarEvent, DateRange, EventTiming, IsoDate } from "../types/calendar";

export const MINUTE = 60_000;
export const HOUR = 60 * MINUTE;

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** Local calendar date of `d` as `YYYY-MM-DD`. */
export function toIsoDate(d: Date): IsoDate {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Local midnight of an ISO date. Throws on malformed input. */
export function parseIsoDate(value: IsoDate): Date {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!m) throw new RangeError(`Invalid date: ${value}`);
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (toIsoDate(d) !== value) throw new RangeError(`Invalid date: ${value}`);
  return d;
}

/** `HH:MM` in local time. */
export function toTimeString(d: Date): string {
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Combine a local date and `HH:MM` into a Date; null when malformed. */
export function combineDateTime(date: IsoDate, time: string): Date | null {
  const t = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
  if (!t) return null;
  const hours = Number(t[1]);
  const minutes = Number(t[2]);
  if (hours > 23 || minutes > 59) return null;
  let d: Date;
  try {
    d = parseIsoDate(date.trim());
  } catch {
    return null;
  }
  // Calendar arithmetic (not fixed milliseconds) so DST days stay correct.
  d.setHours(hours, minutes, 0, 0);
  return d;
}

/** Add whole calendar days, preserving local wall-clock time across DST. */
export function addDays(d: Date, days: number): Date {
  const r = new Date(d);
  r.setDate(r.getDate() + days);
  return r;
}

export function addIsoDays(value: IsoDate, days: number): IsoDate {
  return toIsoDate(addDays(parseIsoDate(value), days));
}

/** Local midnight on the first day of the month `months` after `d`'s month. */
export function monthStart(d: Date, months = 0): Date {
  return new Date(d.getFullYear(), d.getMonth() + months, 1);
}

export function localTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

/**
 * Instant bounds of an event in the current local zone, used for range
 * queries and rendering. All-day dates map to local midnights.
 */
export function timingBounds(timing: EventTiming): DateRange {
  if (timing.kind === "timed") return { start: timing.start, end: timing.end };
  return {
    start: parseIsoDate(timing.startDate).getTime(),
    end: parseIsoDate(timing.endDate).getTime(),
  };
}

export function eventBounds(event: CalendarEvent): DateRange {
  return timingBounds(event.timing);
}

export function overlaps(a: DateRange, b: DateRange): boolean {
  return a.start < b.end && a.end > b.start;
}

/** Returns a message when the timing is invalid, otherwise null. */
export function validateTiming(timing: EventTiming): string | null {
  if (timing.kind === "timed") {
    if (!Number.isFinite(timing.start) || !Number.isFinite(timing.end)) return "Invalid time.";
    if (timing.end <= timing.start) return "The event must end after it starts.";
    return null;
  }
  try {
    parseIsoDate(timing.startDate);
    parseIsoDate(timing.endDate);
  } catch {
    return "Invalid date.";
  }
  if (timing.endDate <= timing.startDate) return "The event must end after it starts.";
  return null;
}

/**
 * Parse a typed time of day into `HH:MM` (24 h). Accepts `9`, `930`, `0930`,
 * `9:30`, `9.30`, `21h30`, and 12-hour forms such as `9:30pm`, `9 a`, `12am`.
 * Returns null when the text is not a valid time.
 */
export function parseTimeInput(text: string): string | null {
  const m = /^(\d{1,2})(?:[:.h]?(\d{2}))?\s*([ap])?\.?\s*m?\.?$/i.exec(text.trim());
  if (!m) return null;
  let hours = Number(m[1]);
  const minutes = m[2] === undefined ? 0 : Number(m[2]);
  const meridiem = m[3]?.toLowerCase();
  if (minutes > 59) return null;
  if (meridiem) {
    if (hours < 1 || hours > 12) return null;
    hours = (hours % 12) + (meridiem === "p" ? 12 : 0);
  } else if (hours > 23) {
    return null;
  }
  return `${pad(hours)}:${pad(minutes)}`;
}

/** Minutes since midnight of an `HH:MM` value. */
export function minutesOfDay(value: string): number {
  const [h = "0", m = "0"] = value.split(":");
  return Number(h) * 60 + Number(m);
}

export function timeFromMinutes(minutes: number): string {
  const m = ((minutes % 1440) + 1440) % 1440;
  return `${pad(Math.floor(m / 60))}:${pad(m % 60)}`;
}

/** Whether the user's locale shows times on a 12-hour clock. */
export function prefers12Hour(): boolean {
  const cycle = new Intl.DateTimeFormat(undefined, { hour: "numeric" }).resolvedOptions().hourCycle;
  return cycle === "h11" || cycle === "h12";
}

/** Display label for an `HH:MM` value, e.g. `9:30 AM` or `09:30`. */
export function formatTimeLabel(value: string, hour12: boolean): string {
  if (!hour12) return value;
  const minutes = minutesOfDay(value);
  const h = Math.floor(minutes / 60);
  const suffix = h < 12 ? "AM" : "PM";
  return `${h % 12 === 0 ? 12 : h % 12}:${pad(minutes % 60)} ${suffix}`;
}

/** Short duration label, e.g. `30 min`, `1 h`, `1 h 30 min`. */
export function formatDuration(minutes: number): string {
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  if (h === 0) return `${m} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}
