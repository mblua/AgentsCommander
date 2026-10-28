// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { isCovered, physicalBounds } from "./WebBrowserView";

const rect = (left: number, top: number, width: number, height: number) =>
  ({ left, top, width, height, right: left + width, bottom: top + height }) as DOMRect;
const floating = (r: DOMRect) => ({ getBoundingClientRect: () => r }) as Element;

describe("WebBrowserView geometry (#3367)", () => {
  it("scales CSS pixels by the device pixel ratio", () => {
    expect(physicalBounds(rect(10, 20, 300, 200), 1.5)).toEqual({
      x: 15,
      y: 30,
      width: 450,
      height: 300,
    });
  });

  it("is covered only by floating elements that overlap the viewport", () => {
    const viewport = rect(100, 100, 400, 300);
    expect(isCovered(viewport, [])).toBe(false);
    expect(isCovered(viewport, [floating(rect(0, 0, 90, 90))])).toBe(false);
    expect(isCovered(viewport, [floating(rect(500, 100, 50, 50))])).toBe(false);
    expect(isCovered(viewport, [floating(rect(450, 350, 100, 100))])).toBe(true);
    expect(isCovered(viewport, [floating(rect(0, 0, 2000, 2000))])).toBe(true);
  });
});
