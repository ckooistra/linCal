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
