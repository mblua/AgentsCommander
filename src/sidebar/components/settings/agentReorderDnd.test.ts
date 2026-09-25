import { describe, expect, it } from "vitest";
import {
  autoScrollDelta,
  EDGE,
  insertionSlot,
  MAX_SPEED,
  reorderedIds,
  reorderIndex,
} from "./agentReorderDnd";

const rect = (top: number, height = 20) => ({ top, height }) as DOMRect;
// Midpoints at 10, 30, 50.
const rows = [rect(0), rect(20), rect(40)];

describe("insertionSlot", () => {
  it("is 0 above the first midpoint", () => {
    expect(insertionSlot(5, rows)).toBe(0);
  });
  it("counts midpoints above the pointer between two midpoints", () => {
    expect(insertionSlot(35, rows)).toBe(2);
  });
  it("is the row count below the last midpoint", () => {
    expect(insertionSlot(99, rows)).toBe(3);
  });
  it("is 0 with no other rows", () => {
    expect(insertionSlot(10, [])).toBe(0);
  });
});

describe("reorderIndex", () => {
  it("returns the slot when the source is above the drop point", () => {
    expect(reorderIndex(0, 2)).toBe(2);
    expect(reorderedIds(["a", "b", "c"], 0, reorderIndex(0, 2))).toEqual(["b", "c", "a"]);
  });
  it("returns the slot when the source is below the drop point", () => {
    expect(reorderIndex(2, 0)).toBe(0);
    expect(reorderedIds(["a", "b", "c"], 2, reorderIndex(2, 0))).toEqual(["c", "a", "b"]);
  });
});

describe("autoScrollDelta", () => {
  const top = 100;
  const bottom = 400;
  it("is zero in the middle", () => {
    expect(autoScrollDelta(250, top, bottom)).toBe(0);
  });
  it("is negative in the top band", () => {
    expect(autoScrollDelta(top + EDGE / 2, top, bottom)).toBe(-MAX_SPEED / 2);
  });
  it("is positive in the bottom band", () => {
    expect(autoScrollDelta(bottom - EDGE / 2, top, bottom)).toBe(MAX_SPEED / 2);
  });
  it("saturates at MAX_SPEED past both edges", () => {
    expect(autoScrollDelta(top - 50, top, bottom)).toBe(-MAX_SPEED);
    expect(autoScrollDelta(bottom + 50, top, bottom)).toBe(MAX_SPEED);
  });
});

describe("reorderedIds", () => {
  const ids = ["a", "b", "c", "d"];
  it("moves down", () => {
    expect(reorderedIds(ids, 1, 2)).toEqual(["a", "c", "b", "d"]);
  });
  it("moves up", () => {
    expect(reorderedIds(ids, 2, 1)).toEqual(["a", "c", "b", "d"]);
  });
  it("moves to index 0", () => {
    expect(reorderedIds(ids, 3, 0)).toEqual(["d", "a", "b", "c"]);
  });
  it("moves to the last index", () => {
    expect(reorderedIds(ids, 0, 3)).toEqual(["b", "c", "d", "a"]);
  });
  it("returns a copy and leaves the input untouched", () => {
    const input = ["a", "b", "c"];
    const out = reorderedIds(input, 0, 2);
    expect(out).not.toBe(input);
    expect(input).toEqual(["a", "b", "c"]);
  });
});
