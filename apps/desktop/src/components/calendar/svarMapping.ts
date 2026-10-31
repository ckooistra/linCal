/**
 * Pure conversions between the domain model and SVAR's event shape.
 * Nothing outside `components/calendar/` should know SVAR's types.
 */
import { addIsoDays, localTimeZone, monthStart, parseIsoDate, timingBounds, toIsoDate } from "../../services/time";
import type {
  Calendar,
  CalendarEvent,
  CalendarViewKind,
  DateRange,
  EventTiming,
  SyncStatus,
} from "../../types/calendar";

export interface SvarEvent {
  id: string;
  start: Date;
  end: Date;
  allDay: boolean;
  text: string;
  calendarId: string;
  status: SyncStatus;
  readOnly: boolean;
}

const cache = new WeakMap<CalendarEvent, SvarEvent>();

/** Map a domain event; results are cached per (immutable) event object. */
export function toSvarEvent(event: CalendarEvent, readOnly: boolean): SvarEvent {
  const cached = cache.get(event);
  if (cached && cached.readOnly === readOnly) return cached;
  const timing = event.timing;
  const mapped: SvarEvent =
    timing.kind === "timed"
      ? {
          id: event.id,
          start: new Date(timing.start),
          end: new Date(timing.end),
          allDay: false,
          text: event.title || "(No title)",
          calendarId: event.calendarId,
          status: event.syncStatus,
          readOnly,
        }
      : {
          id: event.id,
          start: parseIsoDate(timing.startDate),
          end: parseIsoDate(timing.endDate),
          allDay: true,
          text: event.title || "(No title)",
          calendarId: event.calendarId,
          status: event.syncStatus,
          readOnly,
        };
  cache.set(event, mapped);
  return mapped;
}

/**
 * Convert widget start/end back to a domain timing. Timed events keep their
 * original zone; all-day events become exclusive date ranges of at least a day.
 */
export function timingFromWidget(
  previous: EventTiming | null,
  start: Date,
  end: Date,
  allDay: boolean,
): EventTiming {
  if (allDay) {
    const startDate = toIsoDate(start);
    const endMidnight =
      end.getHours() || end.getMinutes() || end.getSeconds() ? addIsoDays(toIsoDate(end), 1) : toIsoDate(end);
    const endDate = endMidnight > startDate ? endMidnight : addIsoDays(startDate, 1);
    return { kind: "all-day", startDate, endDate };
  }
  const timeZone = previous?.kind === "timed" ? previous.timeZone : localTimeZone();
  return { kind: "timed", start: start.getTime(), end: end.getTime(), timeZone };
}

/** Same duration, new start; used when moving events. */
export function shiftTiming(timing: EventTiming, start: Date, allDay: boolean): EventTiming {
  const { start: s, end: e } = timingBounds(timing);
  return timingFromWidget(timing, start, new Date(start.getTime() + (e - s)), allDay);
}

/** CSS-safe identifier for a calendar id. */
export function cssIdent(id: string): string {
  return id.replace(/[^a-zA-Z0-9_-]/g, "_");
}

export function eventClass(event: { calendarId: string; status: SyncStatus; readOnly: boolean }): string {
  let css = `uc-ev uc-cal-${cssIdent(event.calendarId)} uc-st-${event.status}`;
  if (event.readOnly) css += " uc-ro";
  return css;
}

/** Context key for what event content needs from the adapter. */
export const EVENT_CONTEXT = Symbol("uc-event-context");

export interface EventContext {
  /** Name of a calendar, for labels. */
  calendarName(id: string): string;
  /** Open an event (keyboard activation). */
  open(id: string): void;
}

const STATUS_WORDS: Partial<Record<SyncStatus, string>> = {
  "pending-local": "saving",
  "pending-sync": "not synced yet",
  "sync-failed": "sync failed",
  conflict: "conflict",
};

// Formatters are created once: `toLocale…String` builds a new one per call,
// which is slow when every event in view is labelled.
let dayFormat: Intl.DateTimeFormat | null = null;
let timeFormat: Intl.DateTimeFormat | null = null;

/** `10:30` / `10:30 AM` in the user's locale. */
export function formatEventTime(d: Date): string {
  timeFormat ??= new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
  return timeFormat.format(d);
}

const labels = new WeakMap<SvarEvent, { name: string; label: string }>();

/**
 * What a screen reader announces for an event in the grid, e.g.
 * "Dentist, Tuesday 16 June, 10:30 to 11:15, Personal". Cached per
 * (immutable) event object.
 */
export function eventLabel(e: SvarEvent, calendarName: string): string {
  const cached = labels.get(e);
  if (cached?.name === calendarName) return cached.label;
  const label = buildLabel(e, calendarName);
  labels.set(e, { name: calendarName, label });
  return label;
}

function buildLabel(e: SvarEvent, calendarName: string): string {
  dayFormat ??= new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });
  const day = (d: Date) => dayFormat!.format(d);
  const time = formatEventTime;
  let when: string;
  if (e.allDay) {
    const last = new Date(e.end.getTime() - 1);
    when = toIsoDate(last) === toIsoDate(e.start) ? `${day(e.start)}, all day` : `${day(e.start)} to ${day(last)}, all day`;
  } else if (toIsoDate(e.start) === toIsoDate(e.end) || e.end.getTime() - e.start.getTime() <= 0) {
    when = `${day(e.start)}, ${time(e.start)} to ${time(e.end)}`;
  } else {
    when = `${day(e.start)} ${time(e.start)} to ${day(e.end)} ${time(e.end)}`;
  }
  const parts = [e.text, when, calendarName];
  if (e.readOnly) parts.push("read-only");
  const status = STATUS_WORDS[e.status];
  if (status) parts.push(status);
  return parts.join(", ");
}

/** Readable text colour (#fff or #000) for a `#rrggbb` background: at least 4.5:1 for any colour. */
export function textColorFor(hex: string): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
  // Whichever of white and black contrasts more.
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 ? "#fff" : "#000";
}

/**
 * One stylesheet for all calendar colours. Changing a colour rewrites this
 * small string instead of touching any event objects.
 */
export function calendarColorCss(calendars: readonly Calendar[]): string {
  let css = "";
  for (const c of calendars) {
    if (!/^#[0-9a-fA-F]{6}$/.test(c.color)) continue;
    css += `.uc-cal-${cssIdent(c.id)}{--wx-color-primary:${c.color};--wx-color-primary-font:${textColorFor(c.color)};--uc-cal-color:${c.color}}`;
  }
  return css;
}

/**
 * Month-aligned window handed to the widget: the visible range plus one
 * month either side, so adjacent navigation renders from data already
 * present while the next prefetch runs.
 */
export function prefetchWindow(range: DateRange): DateRange {
  const first = new Date(range.start);
  const last = new Date(range.end - 1);
  return {
    start: monthStart(first, -1).getTime(),
    end: monthStart(last, 2).getTime(),
  };
}

export function isViewKind(value: string): value is CalendarViewKind {
  return value === "month" || value === "week" || value === "day";
}
