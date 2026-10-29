import type { Account, Calendar, CalendarEvent, IsoDate } from "../../types/calendar";
import { addDays, addIsoDays, localTimeZone, parseIsoDate, toIsoDate } from "../time";
import { Prng } from "./prng";

/**
 * Deterministic simulated dataset for development and benchmarks.
 *
 * The same `{ seed, eventCount, anchor, timeZone }` always yields identical
 * output. Events are spread over roughly ±6 months around `anchor` and
 * deliberately include the shapes that stress a calendar UI: busy days with
 * many overlapping events, all-day and multi-day events, read-only
 * calendars and pre-expanded recurring series.
 */
export interface DatasetOptions {
  seed: number;
  eventCount: number;
  /** Centre of the generated window, `YYYY-MM-DD`. */
  anchor: IsoDate;
  /** IANA zone recorded on timed events; defaults to the local zone. */
  timeZone?: string;
}

export interface Dataset {
  accounts: Account[];
  calendars: Calendar[];
  events: CalendarEvent[];
}

export const DATASET_SIZES = [100, 1000, 5000] as const;
export const DEFAULT_SEED = 20261029;

const WINDOW_DAYS = 183;
const BUSY_DAY_SPREAD = 30;
const EVENTS_PER_BUSY_DAY = 12;

const ACCOUNTS: Account[] = [
  { id: "acct-personal", name: "Personal (simulated CalDAV)", provider: "simulated" },
  { id: "acct-work", name: "Work (simulated Google)", provider: "simulated" },
  { id: "acct-shared", name: "Shared (simulated ICS)", provider: "simulated" },
];

const CALENDARS: Calendar[] = [
  { id: "cal-personal", accountId: "acct-personal", name: "Personal", color: "#3b82f6", readOnly: false },
  { id: "cal-family", accountId: "acct-personal", name: "Family", color: "#10b981", readOnly: false },
  { id: "cal-fitness", accountId: "acct-personal", name: "Fitness", color: "#f59e0b", readOnly: false },
  { id: "cal-work", accountId: "acct-work", name: "Work", color: "#8b5cf6", readOnly: false },
  { id: "cal-team", accountId: "acct-work", name: "Team", color: "#ec4899", readOnly: false },
  { id: "cal-projects", accountId: "acct-work", name: "Projects", color: "#06b6d4", readOnly: false },
  { id: "cal-holidays", accountId: "acct-shared", name: "Holidays", color: "#ef4444", readOnly: true },
  { id: "cal-birthdays", accountId: "acct-shared", name: "Birthdays", color: "#84cc16", readOnly: true },
];

const TIMED_TOPICS: Record<string, readonly string[]> = {
  "cal-personal": ["Dentist", "Haircut", "Coffee with Sam", "Call the bank", "Book club", "Car service"],
  "cal-family": ["School pickup", "Parents' evening", "Grocery run", "Piano lesson", "Movie night"],
  "cal-fitness": ["Run", "Yoga", "Swimming", "Climbing", "Cycling"],
  "cal-work": ["Design review", "Budget sync", "Customer call", "Hiring panel", "Quarterly planning"],
  "cal-team": ["Sprint planning", "Retro", "Pairing session", "Demo", "Architecture chat"],
  "cal-projects": ["Milestone check", "Release prep", "Bug triage", "Roadmap", "Spec writing"],
};
const TIMED_CALENDARS = Object.keys(TIMED_TOPICS);
const HOLIDAYS = ["Public holiday", "Bank holiday", "National day", "Festival"];
const NAMES = ["Alex", "Jordan", "Robin", "Morgan", "Casey", "Taylor", "Jamie", "Riley"];
const LOCATIONS = ["", "", "Room 4.12", "Online", "Café Central", "Gym", "Home", "Office"];
const DURATIONS_MIN = [15, 30, 30, 45, 60, 60, 60, 90, 120, 180];
// Busier mid-morning and early afternoon, sparse evenings.
const START_HOURS = [7, 8, 9, 9, 10, 10, 11, 11, 12, 13, 14, 14, 15, 15, 16, 17, 18, 19, 20];

interface SeriesSpec {
  id: string;
  calendarId: string;
  title: string;
  /** Days of week (0 = Sunday). */
  weekdays: readonly number[];
  hour: number;
  minute: number;
  durationMin: number;
}

const SERIES: SeriesSpec[] = [
  { id: "series-standup", calendarId: "cal-team", title: "Team stand-up", weekdays: [1, 2, 3, 4, 5], hour: 9, minute: 30, durationMin: 15 },
  { id: "series-gym", calendarId: "cal-fitness", title: "Gym", weekdays: [1, 3, 5], hour: 18, minute: 0, durationMin: 60 },
  { id: "series-1on1", calendarId: "cal-work", title: "1:1 with manager", weekdays: [2], hour: 14, minute: 0, durationMin: 30 },
  { id: "series-dinner", calendarId: "cal-family", title: "Family dinner", weekdays: [0], hour: 19, minute: 0, durationMin: 90 },
];

export function simulatedAccounts(): Account[] {
  return ACCOUNTS.map((a) => ({ ...a }));
}

export function simulatedCalendars(): Calendar[] {
  return CALENDARS.map((c) => ({ ...c }));
}

export function generateDataset(options: DatasetOptions): Dataset {
  const { seed, eventCount, anchor } = options;
  if (!Number.isInteger(eventCount) || eventCount < 0) {
    throw new RangeError("eventCount must be a non-negative integer");
  }
  const timeZone = options.timeZone ?? localTimeZone();
  const rng = new Prng(seed);
  const anchorDate = parseIsoDate(anchor);
  const events: CalendarEvent[] = [];
  let seq = 0;
  const nextId = () => `sim-${seed}-${seq++}`;

  const push = (e: Omit<CalendarEvent, "id" | "syncStatus" | "revision" | "updatedAt">) => {
    events.push({ ...e, id: nextId(), syncStatus: "synced", revision: 1, updatedAt: 0 });
  };

  const timed = (
    calendarId: string,
    title: string,
    day: Date,
    hour: number,
    minute: number,
    durationMin: number,
    seriesId: string | null,
  ) => {
    // Wall-clock construction, so a 09:30 meeting stays 09:30 across DST.
    const start = new Date(day.getFullYear(), day.getMonth(), day.getDate(), hour, minute);
    const end = new Date(start);
    end.setMinutes(end.getMinutes() + durationMin);
    push({
      calendarId,
      title,
      description: "",
      location: rng.pick(LOCATIONS),
      timing: { kind: "timed", start: start.getTime(), end: end.getTime(), timeZone },
      seriesId,
    });
  };

  const allDayCount = Math.round(eventCount * 0.08);
  const recurringCount = Math.round(eventCount * 0.1);
  const burstCount = Math.round(eventCount * 0.12);
  const scatteredCount = eventCount - allDayCount - recurringCount - burstCount;

  // Recurring series occurrences, densest around the anchor date.
  const occurrences: { spec: SeriesSpec; day: Date; distance: number }[] = [];
  for (let offset = -WINDOW_DAYS; offset <= WINDOW_DAYS; offset++) {
    const day = addDays(anchorDate, offset);
    for (const spec of SERIES) {
      if (spec.weekdays.includes(day.getDay())) {
        occurrences.push({ spec, day, distance: Math.abs(offset) });
      }
    }
  }
  occurrences.sort((a, b) => a.distance - b.distance || a.day.getTime() - b.day.getTime());
  for (const { spec, day } of occurrences.slice(0, recurringCount)) {
    timed(spec.calendarId, spec.title, day, spec.hour, spec.minute, spec.durationMin, spec.id);
  }

  // Busy days: many overlapping events between 09:00 and 17:00.
  const busyDays = Math.max(1, Math.ceil(burstCount / EVENTS_PER_BUSY_DAY));
  for (let i = 0; i < burstCount; i++) {
    const dayIndex = i % busyDays;
    // Deterministic placement of busy days within ±BUSY_DAY_SPREAD of the anchor.
    const offset = ((dayIndex * 7919) % (2 * BUSY_DAY_SPREAD + 1)) - BUSY_DAY_SPREAD;
    const calendarId = rng.pick(TIMED_CALENDARS);
    timed(
      calendarId,
      rng.pick(TIMED_TOPICS[calendarId] ?? ["Meeting"]),
      addDays(anchorDate, offset),
      rng.int(9, 16),
      rng.pick([0, 15, 30, 45]),
      rng.pick([30, 45, 60, 90]),
      null,
    );
  }

  // Scattered timed events across the whole window.
  for (let i = 0; i < scatteredCount; i++) {
    const calendarId = rng.pick(TIMED_CALENDARS);
    timed(
      calendarId,
      rng.pick(TIMED_TOPICS[calendarId] ?? ["Meeting"]),
      addDays(anchorDate, rng.int(-WINDOW_DAYS, WINDOW_DAYS)),
      rng.pick(START_HOURS),
      rng.pick([0, 15, 30, 45]),
      rng.pick(DURATIONS_MIN),
      null,
    );
  }

  // All-day and multi-day events, including read-only calendars.
  for (let i = 0; i < allDayCount; i++) {
    const startDate = toIsoDate(addDays(anchorDate, rng.int(-WINDOW_DAYS, WINDOW_DAYS)));
    const roll = rng.next();
    let calendarId: string;
    let title: string;
    let days = 1;
    if (roll < 0.25) {
      calendarId = "cal-holidays";
      title = rng.pick(HOLIDAYS);
    } else if (roll < 0.5) {
      calendarId = "cal-birthdays";
      title = `${rng.pick(NAMES)}'s birthday`;
    } else if (roll < 0.75) {
      calendarId = "cal-work";
      title = rng.pick(["Conference", "Offsite", "Training", "Deadline"]);
      days = rng.int(1, 4);
    } else {
      calendarId = "cal-personal";
      title = rng.pick(["Vacation", "Trip", "Day off", "Moving day"]);
      days = rng.int(1, 5);
    }
    push({
      calendarId,
      title,
      description: "",
      location: "",
      timing: { kind: "all-day", startDate, endDate: addIsoDays(startDate, days) },
      seriesId: null,
    });
  }

  return { accounts: simulatedAccounts(), calendars: simulatedCalendars(), events };
}
