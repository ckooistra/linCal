import type { CalendarEvent, DateRange, IsoDate } from "../types/calendar";
import { addDays, eventBounds, formatTimeLabel, parseIsoDate, toIsoDate, toTimeString } from "./time";

export interface AgendaItem {
  event: CalendarEvent;
  /** When the event happens on this day, e.g. `09:00 – 10:00`, `All day`, `Until 14:00`. */
  when: string;
  /** Sort key within the day: all-day items first, then by start. */
  order: number;
}

export interface AgendaDay {
  date: IsoDate;
  items: AgendaItem[];
}

export interface AgendaOptions {
  /** List events spanning several days under each of those days (agenda) or only the first (search results). */
  spread: boolean;
  hour12: boolean;
}

/**
 * Groups events into local days within `range`, in date order. Days without
 * events are left out.
 */
export function agendaDays(events: readonly CalendarEvent[], range: DateRange, options: AgendaOptions): AgendaDay[] {
  const days = new Map<IsoDate, AgendaItem[]>();
  const time = (ms: number) => formatTimeLabel(toTimeString(new Date(ms)), options.hour12);
  for (const event of events) {
    const bounds = eventBounds(event);
    const first = startOfDay(Math.max(bounds.start, range.start));
    // Zero-length events still appear on their day.
    const end = Math.min(Math.max(bounds.end, bounds.start + 1), range.end);
    for (let day = first; day.getTime() < end; day = addDays(day, 1)) {
      const dayStart = day.getTime();
      const dayEnd = addDays(day, 1).getTime();
      let when: string;
      let order: number;
      if (event.timing.kind === "all-day" || (bounds.start <= dayStart && bounds.end >= dayEnd)) {
        when = "All day";
        order = -1;
      } else if (bounds.start < dayStart) {
        when = `Until ${time(bounds.end)}`;
        order = 0;
      } else if (bounds.end > dayEnd) {
        when = `From ${time(bounds.start)}`;
        order = bounds.start - dayStart;
      } else {
        when = bounds.end > bounds.start ? `${time(bounds.start)} – ${time(bounds.end)}` : time(bounds.start);
        order = bounds.start - dayStart;
      }
      const key = toIsoDate(day);
      let items = days.get(key);
      if (!items) days.set(key, (items = []));
      items.push({ event, when, order });
      if (!options.spread) break;
    }
  }
  return [...days.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([date, items]) => ({
      date,
      items: items.sort((a, b) => a.order - b.order || a.event.title.localeCompare(b.event.title)),
    }));
}

function startOfDay(ms: number): Date {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d;
}

/** Heading for an agenda day, e.g. `Saturday, 31 October 2026`. */
export function dayHeading(date: IsoDate, locale?: string): string {
  return parseIsoDate(date).toLocaleDateString(locale, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}
