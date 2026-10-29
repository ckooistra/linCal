import { describe, expect, it } from "vitest";
import { mergeRange, rangeCovered } from "../stores/ranges";

describe("mergeRange", () => {
  it("keeps disjoint ranges sorted", () => {
    let r = mergeRange([], { start: 10, end: 20 });
    r = mergeRange(r, { start: 0, end: 5 });
    r = mergeRange(r, { start: 30, end: 40 });
    expect(r).toEqual([
      { start: 0, end: 5 },
      { start: 10, end: 20 },
      { start: 30, end: 40 },
    ]);
  });

  it("merges overlapping and touching ranges", () => {
    const r = mergeRange(
      [
        { start: 0, end: 5 },
        { start: 10, end: 20 },
        { start: 30, end: 40 },
      ],
      { start: 5, end: 30 },
    );
    expect(r).toEqual([{ start: 0, end: 40 }]);
  });

  it("reports coverage only for fully contained ranges", () => {
    const r = [{ start: 0, end: 10 }];
    expect(rangeCovered(r, { start: 2, end: 8 })).toBe(true);
    expect(rangeCovered(r, { start: 5, end: 15 })).toBe(false);
  });
});
