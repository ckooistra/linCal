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

/** Readable text colour (#fff or near-black) for a `#rrggbb` background. */
export function textColorFor(hex: string): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * channel((n >> 16) & 255) + 0.7152 * channel((n >> 8) & 255) + 0.0722 * channel(n & 255);
  // Pick whichever of white / #111 (luminance ≈ 0.0056) contrasts more.
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.0556 ? "#fff" : "#111";
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
