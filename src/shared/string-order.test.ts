import { describe, expect, it } from "vitest";
import { compareCodeUnits } from "./string-order";

describe("compareCodeUnits", () => {
  it("returns -1, 1 and 0", () => {
    expect(compareCodeUnits("a", "b")).toBe(-1);
    expect(compareCodeUnits("b", "a")).toBe(1);
    expect(compareCodeUnits("a", "a")).toBe(0);
  });

  it("matches comparator-less sort order", () => {
    const arr = ["b", "B", "_x", "a10", "a9", "", "Z", "é", "e", "😀", "￿"];
    expect([...arr].sort(compareCodeUnits)).toEqual([...arr].sort());
  });
});
