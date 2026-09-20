import { describe, expect, it } from "vitest";
import { FILTER_DEBOUNCE_MS, MAX_PIDS, parsePidFilter } from "./pid-filter";

// #2245 - the PID grammar, with no DOM in sight. Every case below is a decision
// rule of the plan rather than an illustration: the separators, the u32 bound,
// the leading-zero normalization, the dedup order, the truncation flag and the
// empty parse are each what one mutant in `parsePidFilter` would change.
describe("parsePidFilter", () => {
  it("parses a single PID", () => {
    expect(parsePidFilter("4242")).toEqual({
      pids: [4242],
      rejected: [],
      truncated: false,
    });
  });

  it("normalizes separators, blanks, duplicates and leading zeros", () => {
    // Commas, semicolons and runs of whitespace all separate; an empty token
    // (the doubled comma, the trailing one) is dropped rather than rejected.
    expect(parsePidFilter(" 4242 ,, 5120 ; 4242 ").pids).toEqual([4242, 5120]);
    // `04242` and `4242` are the same PID because the token becomes a Number.
    expect(parsePidFilter("04242, 5120").pids).toEqual([4242, 5120]);
  });

  it("applies the valid PIDs and names the rejected tokens in order", () => {
    const parsed = parsePidFilter("4242, abc, 42a, -5");
    expect(parsed.pids).toEqual([4242]);
    // Verbatim and in appearance order: the notice names what the user typed.
    expect(parsed.rejected).toEqual(["abc", "42a", "-5"]);
    expect(parsed.truncated).toBe(false);
  });

  it("holds the u32 bound and accepts zero", () => {
    expect(parsePidFilter("4294967295").pids).toEqual([4294967295]);
    const overflow = parsePidFilter("4294967296");
    expect(overflow.pids).toEqual([]);
    expect(overflow.rejected).toEqual(["4294967296"]);
    // Representable in u32, so rejecting it would be an invented rule. It
    // simply matches nothing.
    expect(parsePidFilter("0").pids).toEqual([0]);
  });

  it("truncates beyond MAX_PIDS distinct valid PIDs", () => {
    const forty = Array.from({ length: 40 }, (_, i) => 1000 + i);
    const parsed = parsePidFilter(forty.join(","));
    expect(parsed.pids).toHaveLength(MAX_PIDS);
    expect(parsed.pids[0]).toBe(1000);
    expect(parsed.pids[MAX_PIDS - 1]).toBe(1000 + MAX_PIDS - 1);
    expect(parsed.truncated).toBe(true);
  });

  it("returns an asserted empty parse for empty and separator-only input", () => {
    // Zero is a typed state here, not a fallback: an empty parse is what tells
    // the caller the PID filter does not participate at all.
    for (const text of ["", " , ; "]) {
      expect(parsePidFilter(text)).toEqual({
        pids: [],
        rejected: [],
        truncated: false,
      });
    }
  });

  it("pins the exported constants", () => {
    // The debounce value lives here because a rendered test cannot distinguish
    // 200 ms from 20 ms without sleeping for the difference.
    expect(FILTER_DEBOUNCE_MS).toBe(200);
    expect(MAX_PIDS).toBe(32);
  });
});
