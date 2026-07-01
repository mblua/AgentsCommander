import { describe, it, expect } from "vitest";
import { drawOverlay, SELECTION_BORDER_CSS } from "./render";
import { magnifierSourceRect } from "./selection";

interface RecordedCall {
  op: string;
  args: number[];
  lineWidth: number;
  smoothing: boolean;
}

/** A minimal recording stand-in for CanvasRenderingContext2D: every draw op is
 *  captured with a snapshot of the current lineWidth / imageSmoothingEnabled, so
 *  tests can assert the exact sequence and DPI-scaled sizes without real pixels
 *  (jsdom's canvas is a no-op stub). */
function recordingCtx() {
  const calls: RecordedCall[] = [];
  const ctx: Record<string, unknown> = {
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 0,
    imageSmoothingEnabled: true,
  };
  const rec =
    (op: string) =>
    (...args: unknown[]) => {
      calls.push({
        op,
        args: args.filter((a): a is number => typeof a === "number"),
        lineWidth: ctx.lineWidth as number,
        smoothing: ctx.imageSmoothingEnabled as boolean,
      });
    };
  for (const op of [
    "clearRect",
    "drawImage",
    "fillRect",
    "strokeRect",
    "save",
    "restore",
    "beginPath",
    "closePath",
    "arc",
    "clip",
    "stroke",
    "moveTo",
    "lineTo",
  ]) {
    ctx[op] = rec(op);
  }
  return {
    ctx: ctx as unknown as CanvasRenderingContext2D,
    calls,
    drawImages: () => calls.filter((c) => c.op === "drawImage"),
  };
}

const IMAGE = {} as CanvasImageSource;
const MODEL = { width: 1920, height: 1080, physicalPerCss: 1 };

describe("drawOverlay", () => {
  it("draws the frozen image then a dark mask when there is no selection or hover", () => {
    const { ctx, calls, drawImages } = recordingCtx();
    drawOverlay(ctx, IMAGE, MODEL, null, null);

    // Exactly one drawImage (the base frame), 5 numeric args = (image,0,0,w,h).
    expect(drawImages()).toHaveLength(1);
    expect(drawImages()[0].args).toEqual([0, 0, 1920, 1080]);
    // A full-canvas dark mask fill.
    expect(calls.some((c) => c.op === "fillRect")).toBe(true);
    // No selection border, no magnifier clip.
    expect(calls.some((c) => c.op === "strokeRect")).toBe(false);
    expect(calls.some((c) => c.op === "clip")).toBe(false);
  });

  it("un-dims the selection region and strokes a border sized by physicalPerCss", () => {
    const { ctx, calls, drawImages } = recordingCtx();
    const selection = { x: 100, y: 200, width: 300, height: 150 };
    drawOverlay(ctx, IMAGE, { ...MODEL, physicalPerCss: 2 }, selection, null);

    // Base draw + one un-dim draw of the selection source→dest (9 numeric args).
    const draws = drawImages();
    expect(draws).toHaveLength(2);
    expect(draws[1].args).toEqual([100, 200, 300, 150, 100, 200, 300, 150]);

    // Border stroked over the selection, width scaled by the DPI ratio.
    const strokeRect = calls.find((c) => c.op === "strokeRect");
    expect(strokeRect?.args).toEqual([100, 200, 300, 150]);
    expect(strokeRect?.lineWidth).toBe(SELECTION_BORDER_CSS * 2);
  });

  it("does NOT un-dim a zero-area selection", () => {
    const { ctx, drawImages } = recordingCtx();
    drawOverlay(ctx, IMAGE, MODEL, { x: 10, y: 10, width: 0, height: 5 }, null);
    expect(drawImages()).toHaveLength(1); // base only
  });

  it("samples the magnifier from the clamped source rect with smoothing OFF", () => {
    const { ctx, calls, drawImages } = recordingCtx();
    const hover = { x: 5, y: 5 }; // near the top-left corner → source clamps to 0,0
    drawOverlay(ctx, IMAGE, MODEL, null, hover);

    // Clip to a circle before sampling.
    expect(calls.some((c) => c.op === "clip")).toBe(true);

    // The magnifier drawImage is a 9-arg blit whose SOURCE matches the pure
    // magnifierSourceRect helper, and it must run with imageSmoothingEnabled
    // false (pixelated zoom).
    const src = magnifierSourceRect(hover, 32, MODEL.width, MODEL.height);
    const magnifierDraw = drawImages().find(
      (c) => c.args.length === 8 && c.args[0] === src.x && c.args[1] === src.y
    );
    expect(magnifierDraw).toBeTruthy();
    expect(magnifierDraw!.args.slice(0, 4)).toEqual([
      src.x,
      src.y,
      src.width,
      src.height,
    ]);
    expect(magnifierDraw!.smoothing).toBe(false);
    // A save/restore brackets the clipped sampling.
    expect(calls.some((c) => c.op === "save")).toBe(true);
    expect(calls.some((c) => c.op === "restore")).toBe(true);
  });
});
