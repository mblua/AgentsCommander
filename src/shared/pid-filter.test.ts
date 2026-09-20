import { describe, expect, it } from "vitest";
import { FILTER_DEBOUNCE_MS, MAX_PIDS, parsePidFilter } from "./pid-filter";

describe("parsePidFilter", () => {
  it("parses a single PID", () => {
    expect(parsePidFilter("4242")).toEqual({
      pids: [4242],
      rejected: [],
      truncated: false,
    });
  });

  it("normalizes separators, empty tokens and leading zeros", () => {
    expect(parsePidFilter(" 4242 ,, 5120 ; 4242 ").pids).toEqual([4242, 5120]);
    expect(parsePidFilter("04242, 5120").pids).toEqual([4242, 5120]);
  });

  it("keeps rejected tokens verbatim in appearance order", () => {
    expect(parsePidFilter("4242, abc, 42a, -5")).toEqual({
      pids: [4242],
      rejected: ["abc", "42a", "-5"],
      truncated: false,
    });
  });

  it("accepts the u32 boundary, rejects above it, and accepts zero", () => {
    expect(parsePidFilter("4294967295").pids).toEqual([4294967295]);
    expect(parsePidFilter("4294967296")).toEqual({
      pids: [],
      rejected: ["4294967296"],
      truncated: false,
    });
    expect(parsePidFilter("0").pids).toEqual([0]);
  });

  it("truncates after MAX_PIDS distinct PIDs", () => {
    const text = Array.from({ length: 40 }, (_, i) => `${1000 + i}`).join(",");
    const parsed = parsePidFilter(text);
    expect(parsed.pids).toHaveLength(MAX_PIDS);
    expect(parsed.truncated).toBe(true);
    expect(parsed.rejected).toEqual([]);
  });

  it("treats empty input as a typed no-op state", () => {
    for (const text of ["", " , ; "]) {
      expect(parsePidFilter(text)).toEqual({
        pids: [],
        rejected: [],
        truncated: false,
      });
    }
  });

  it("pins the debounce and cap constants", () => {
    expect(FILTER_DEBOUNCE_MS).toBe(200);
    expect(MAX_PIDS).toBe(32);
  });
});
