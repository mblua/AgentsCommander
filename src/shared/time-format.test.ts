import { describe, expect, it } from "vitest";
import { formatClockTime } from "./time-format";

/**
 * #1171 test 76: `at` parses with `new Date(value)` and the clock is zero-padded 24-hour
 * `HH:MM:SS`, not the viewer's locale.
 */
describe("formatClockTime (#1171)", () => {
  it("zero-pads to a fixed-width 24-hour clock", () => {
    // Built from local parts so the assertion does not depend on the runner's timezone;
    // what is being pinned is the FORMAT, not the conversion.
    const morning = new Date(2026, 6, 30, 9, 5, 4);
    expect(formatClockTime(morning.toISOString())).toBe("09:05:04");
  });

  it("uses 24-hour and never AM/PM", () => {
    const afternoon = new Date(2026, 6, 30, 14, 31, 5);
    expect(formatClockTime(afternoon.toISOString())).toBe("14:31:05");
    expect(formatClockTime(afternoon.toISOString())).not.toMatch(/[AP]M/i);
  });

  it("keeps every row the same width, which is what the column needs", () => {
    const values = [
      new Date(2026, 0, 1, 0, 0, 0),
      new Date(2026, 0, 1, 23, 59, 59),
      new Date(2026, 0, 1, 7, 8, 9),
    ].map((d) => formatClockTime(d.toISOString()));
    expect(new Set(values.map((v) => v.length))).toEqual(new Set([8]));
  });

  it("accepts the RFC3339 the backend actually sends", () => {
    expect(formatClockTime("2026-07-30T22:31:05Z")).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("degrades to a placeholder of the same width rather than throwing", () => {
    expect(formatClockTime(null)).toBe("--:--:--");
    expect(formatClockTime(undefined)).toBe("--:--:--");
    expect(formatClockTime("")).toBe("--:--:--");
    expect(formatClockTime("not a date")).toBe("--:--:--");
  });
});
