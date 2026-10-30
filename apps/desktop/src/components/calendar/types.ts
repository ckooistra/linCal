import type { CalendarViewKind, DateRange, EventId, EventTiming } from "../../types/calendar";

/**
 * Contract between the application and whatever widget renders the
 * calendar. `CalendarAdapter.svelte` implements it for SVAR; a replacement
 * widget only needs to implement the same props and controller.
 */
export interface CalendarViewController {
  setView(view: CalendarViewKind): void;
  navigate(direction: "previous" | "next" | "today"): void;
  goToDate(date: Date): void;
}

export interface CalendarViewState {
  view: CalendarViewKind;
  /** Human-readable label of the visible range, e.g. "October 2026". */
  label: string;
  range: DateRange;
}

export type EditorRequest =
  | { mode: "edit"; id: EventId }
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
