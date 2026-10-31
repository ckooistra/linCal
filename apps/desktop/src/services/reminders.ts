/** Reminder choices offered by the editor, as minutes before the start. */
export const REMINDER_PRESETS: readonly number[] = [0, 5, 10, 15, 30, 60, 120, 1440, 2880, 10080];

/** Most reminders per event (as in the Rust core). */
export const MAX_REMINDERS = 5;

/** A reminder for display, e.g. `15 minutes before`, `1 day before`, `At start`. */
export function reminderLabel(minutes: number): string {
  if (minutes === 0) return "At start";
  const parts: string[] = [];
  const weeks = minutes % 10080 === 0 ? minutes / 10080 : 0;
  if (weeks) return `${weeks} ${weeks === 1 ? "week" : "weeks"} before`;
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days) parts.push(`${days} ${days === 1 ? "day" : "days"}`);
  if (hours) parts.push(`${hours} ${hours === 1 ? "hour" : "hours"}`);
  if (mins) parts.push(`${mins} ${mins === 1 ? "minute" : "minutes"}`);
  return `${parts.join(" ")} before`;
}

/** Sorted, without duplicates (as stored). */
export function normalizeReminders(reminders: readonly number[]): number[] {
  return [...new Set(reminders)].sort((a, b) => a - b);
}
