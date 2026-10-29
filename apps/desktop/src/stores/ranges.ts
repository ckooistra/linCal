import type { DateRange } from "../types/calendar";

/** Insert `range` into a sorted, non-overlapping list, merging touching ranges. */
export function mergeRange(ranges: readonly DateRange[], range: DateRange): DateRange[] {
  const result: DateRange[] = [];
  let merged = { ...range };
  let inserted = false;
  for (const r of ranges) {
    if (r.end < merged.start) {
      result.push(r);
    } else if (r.start > merged.end) {
      if (!inserted) {
        result.push(merged);
        inserted = true;
      }
      result.push(r);
    } else {
      merged = { start: Math.min(r.start, merged.start), end: Math.max(r.end, merged.end) };
    }
  }
  if (!inserted) result.push(merged);
  return result;
}

export function rangeCovered(ranges: readonly DateRange[], range: DateRange): boolean {
  return ranges.some((r) => r.start <= range.start && r.end >= range.end);
}
