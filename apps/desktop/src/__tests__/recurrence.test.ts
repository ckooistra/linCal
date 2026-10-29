import { describe, expect, it } from "vitest";
import { REPEAT_PRESETS, repeatOptions } from "../services/recurrence";

describe("repeatOptions", () => {
  it("returns the presets for known or empty rules", () => {
    expect(repeatOptions(null)).toEqual(REPEAT_PRESETS);
    expect(repeatOptions("FREQ=WEEKLY")).toEqual(REPEAT_PRESETS);
  });

  it("keeps an unknown rule selectable", () => {
    const options = repeatOptions("FREQ=MONTHLY;BYDAY=2TU");
    expect(options.at(-1)).toEqual({ label: "Custom (FREQ=MONTHLY;BYDAY=2TU)", rrule: "FREQ=MONTHLY;BYDAY=2TU" });
  });
});
