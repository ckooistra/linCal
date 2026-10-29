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
