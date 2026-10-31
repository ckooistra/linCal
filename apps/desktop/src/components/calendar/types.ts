import type { AppView, CalendarEvent, CalendarViewKind, DateRange, EventId, EventTiming } from "../../types/calendar";

/**
 * Contract between the application and whatever widget renders the
 * calendar. `CalendarAdapter.svelte` implements it for SVAR; a replacement
 * widget only needs to implement the same props and controller.
 */
export interface CalendarViewController {
  /** Switch between grid views (the agenda ignores it). */
  setView(view: CalendarViewKind): void;
  navigate(direction: "previous" | "next" | "today"): void;
  goToDate(date: Date): void;
}

export interface CalendarViewState {
  view: AppView;
  /** Human-readable label of the visible range, e.g. "October 2026". */
  label: string;
  range: DateRange;
}

export type EditorRequest =
  /** `event` opens an occurrence that may not be cached yet (e.g. a search result). */
  | { mode: "edit"; id: EventId; event?: CalendarEvent }
  | { mode: "create"; timing: EventTiming };

export interface MoveRequest {
  id: EventId;
  timing: EventTiming;
}

/** Time range of the week/day grid. */
export type DayHours = "work" | "full";

export const DAY_HOURS: Record<DayHours, { startHour: number; endHour: number }> = {
  work: { startHour: 8, endHour: 18 },
  full: { startHour: 0, endHour: 24 },
};

export function parseDayHours(value: unknown): DayHours {
  return value === "work" ? "work" : "full";
}
