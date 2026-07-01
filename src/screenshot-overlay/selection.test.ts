import { describe, it, expect } from "vitest";
import {
  clampSelectionToBounds,
  clientToImagePoint,
  magnifierSourceRect,
  normalizeSelection,
} from "./selection";

describe("normalizeSelection", () => {
  it("normalizes a top-left → bottom-right drag", () => {
    expect(normalizeSelection({ x: 10, y: 20 }, { x: 40, y: 60 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("normalizes a bottom-right → top-left drag (both axes reversed)", () => {
    expect(normalizeSelection({ x: 40, y: 60 }, { x: 10, y: 20 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("normalizes mixed-direction drags", () => {
    expect(normalizeSelection({ x: 40, y: 20 }, { x: 10, y: 60 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
    expect(normalizeSelection({ x: 10, y: 60 }, { x: 40, y: 20 })).toEqual({
      x: 10,
      y: 20,
      width: 30,
      height: 40,
    });
  });

  it("yields a zero-size rect for a click with no movement", () => {
    expect(normalizeSelection({ x: 5, y: 5 }, { x: 5, y: 5 })).toEqual({
      x: 5,
      y: 5,
      width: 0,
      height: 0,
    });
  });
});

describe("clientToImagePoint", () => {
  it("maps 1:1 when the display matches the image size", () => {
    const rect = { left: 0, top: 0, width: 1920, height: 1080 };
    expect(clientToImagePoint({ x: 960, y: 540 }, rect, 1920, 1080)).toEqual({
      x: 960,
      y: 540,
    });
  });

  it("scales up when the canvas is DISPLAYED SMALLER than the image (HiDPI)", () => {
    // 3840x2160 backing store shown at 1920x1080 CSS px (200% monitor): a click at
    // the CSS center must map to the physical center, proving the mapping uses the
    // bounding rect, not a 1:1 assumption.
    const rect = { left: 0, top: 0, width: 1920, height: 1080 };
    expect(clientToImagePoint({ x: 960, y: 540 }, rect, 3840, 2160)).toEqual({
      x: 1920,
      y: 1080,
    });
    // A corner click maps to the far physical corner.
    expect(clientToImagePoint({ x: 1920, y: 1080 }, rect, 3840, 2160)).toEqual({
      x: 3840,
      y: 2160,
    });
  });

  it("accounts for a non-zero rect origin (offset canvas)", () => {
    const rect = { left: 100, top: 50, width: 1000, height: 1000 };
    expect(clientToImagePoint({ x: 600, y: 550 }, rect, 2000, 2000)).toEqual({
      x: 1000,
      y: 1000,
    });
  });

  it("returns the origin instead of NaN for a zero-sized rect", () => {
    const rect = { left: 0, top: 0, width: 0, height: 0 };
    expect(clientToImagePoint({ x: 10, y: 10 }, rect, 1920, 1080)).toEqual({
      x: 0,
      y: 0,
    });
  });
});

describe("magnifierSourceRect", () => {
  it("centers the source square on the point when away from edges", () => {
    expect(magnifierSourceRect({ x: 500, y: 500 }, 32, 1920, 1080)).toEqual({
      x: 484,
      y: 484,
      width: 32,
      height: 32,
    });
  });

  it("clamps at the top-left edge", () => {
    expect(magnifierSourceRect({ x: 0, y: 0 }, 32, 1920, 1080)).toEqual({
      x: 0,
      y: 0,
      width: 32,
      height: 32,
    });
  });

  it("clamps at the bottom-right edge", () => {
    expect(magnifierSourceRect({ x: 1920, y: 1080 }, 32, 1920, 1080)).toEqual({
      x: 1888,
      y: 1048,
      width: 32,
      height: 32,
    });
  });

  it("shrinks the square to fit an image smaller than the source size", () => {
    expect(magnifierSourceRect({ x: 5, y: 5 }, 32, 10, 10)).toEqual({
      x: 0,
      y: 0,
      width: 10,
      height: 10,
    });
  });
});

describe("clampSelectionToBounds", () => {
  it("leaves an in-bounds selection unchanged", () => {
    expect(
      clampSelectionToBounds({ x: 10, y: 10, width: 100, height: 100 }, 1920, 1080)
    ).toEqual({ x: 10, y: 10, width: 100, height: 100 });
  });

  it("clamps a selection that overruns the far edge before backend confirm", () => {
    // x=3839 + width=2 would be 3841 on a 3840-wide image → backend rejects it.
    expect(
      clampSelectionToBounds({ x: 3839, y: 0, width: 2, height: 2 }, 3840, 2160)
    ).toEqual({ x: 3839, y: 0, width: 1, height: 2 });
  });

  it("clamps negative origins to zero and trims the size", () => {
    expect(
      clampSelectionToBounds({ x: -20, y: -10, width: 50, height: 50 }, 1920, 1080)
    ).toEqual({ x: 0, y: 0, width: 50, height: 50 });
  });

  it("collapses a fully out-of-bounds selection to a zero-size rect at the edge", () => {
    expect(
      clampSelectionToBounds({ x: 5000, y: 5000, width: 100, height: 100 }, 1920, 1080)
    ).toEqual({ x: 1920, y: 1080, width: 0, height: 0 });
  });
});
