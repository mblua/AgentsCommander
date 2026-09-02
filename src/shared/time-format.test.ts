import { describe, expect, it } from "vitest";
import { formatAgentMessageStamp, formatClockTime } from "./time-format";

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

/**
 * #1682 - the terminal status strip's stamp: local `MM-DD HH:MM`, no year and no
 * seconds, and the empty string rather than a placeholder when there is nothing
 * to show. Inputs are built from LOCAL parts so the assertions pin the FORMAT and
 * not the runner's timezone.
 */
describe("formatAgentMessageStamp (#1682)", () => {
  it("renders MM-DD HH:MM from local parts", () => {
    const at = new Date(2026, 7, 31, 21, 29);
    expect(formatAgentMessageStamp(at.toISOString())).toBe("08-31 21:29");
  });

  it("zero-pads both halves", () => {
    const at = new Date(2026, 0, 2, 3, 4);
    expect(formatAgentMessageStamp(at.toISOString())).toBe("01-02 03:04");
  });

  it("uses 24-hour time and never AM/PM", () => {
    const at = new Date(2026, 0, 1, 14, 5);
    expect(formatAgentMessageStamp(at.toISOString())).toBe("01-01 14:05");
    expect(formatAgentMessageStamp(at.toISOString())).not.toMatch(/[AP]M/i);
  });

  it("carries no year and no seconds", () => {
    const values = [
      new Date(2026, 0, 1, 0, 0, 0),
      new Date(2026, 11, 31, 23, 59, 59),
      new Date(2026, 5, 7, 8, 9, 10),
    ].map((d) => formatAgentMessageStamp(d.toISOString()));
    for (const value of values) {
      expect(value).toMatch(/^\d{2}-\d{2} \d{2}:\d{2}$/);
      expect(value).toHaveLength(11);
    }
  });

  it("accepts the RFC3339 the backend actually sends", () => {
    expect(formatAgentMessageStamp("2026-08-31T21:29:07Z")).toMatch(
      /^\d{2}-\d{2} \d{2}:\d{2}$/
    );
  });

  it("renders nothing at all rather than a placeholder", () => {
    expect(formatAgentMessageStamp(null)).toBe("");
    expect(formatAgentMessageStamp(undefined)).toBe("");
    expect(formatAgentMessageStamp("")).toBe("");
    expect(formatAgentMessageStamp("not a date")).toBe("");
  });
});
