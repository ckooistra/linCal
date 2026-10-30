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

/** Options for a select: the presets plus, if needed, the event's own custom rule. */
export function repeatOptions(current: string | null): RepeatPreset[] {
  if (current === null || REPEAT_PRESETS.some((p) => p.rrule === current)) return [...REPEAT_PRESETS];
  // "" is a series given only by extra dates (iCalendar RDATE).
  const label = current === "" ? "On specific dates" : `Custom (${current})`;
  return [...REPEAT_PRESETS, { label, rrule: current }];
}
