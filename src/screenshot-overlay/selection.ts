// #714 Pure geometry helpers for the screenshot overlay. Kept free of DOM/canvas
// so they are unit-testable in jsdom/vitest (which cannot render canvas pixels).
// All coordinates here are PHYSICAL image pixels unless a param is named
// `client` (CSS/viewport pixels from a PointerEvent).

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The subset of `DOMRect` the coordinate mapping needs. Lets tests pass a plain
 *  object instead of constructing a real bounding-client-rect. */
export interface ClientRectLike {
  left: number;
  top: number;
  width: number;
  height: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

/** Top-left origin + non-negative width/height for a drag in ANY direction. */
export function normalizeSelection(start: Point, end: Point): Rect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

/** Map a client (CSS-pixel) point onto physical image coordinates. The canvas is
 *  displayed at some CSS size (`rect`) but backed by `imageWidth × imageHeight`
 *  physical pixels, so the mapping is proportional — this is what makes the crop
 *  DPI-independent (see plan: map via bounding rect, NOT devicePixelRatio). A
 *  zero-sized rect maps to the origin instead of producing NaN. */
export function clientToImagePoint(
  client: Point,
  rect: ClientRectLike,
  imageWidth: number,
  imageHeight: number
): Point {
  const fx = rect.width > 0 ? (client.x - rect.left) / rect.width : 0;
  const fy = rect.height > 0 ? (client.y - rect.top) / rect.height : 0;
  return {
    x: Math.round(fx * imageWidth),
    y: Math.round(fy * imageHeight),
  };
}

/** A square source rect of `sourceSize` physical px centered on `center`, clamped
 *  so it always stays fully inside the image (the magnifier samples from this).
 *  If the image is smaller than `sourceSize` in either axis, the square shrinks
 *  to fit. */
export function magnifierSourceRect(
  center: Point,
  sourceSize: number,
  imageWidth: number,
  imageHeight: number
): Rect {
  const size = Math.max(1, Math.min(sourceSize, imageWidth, imageHeight));
  const x = clamp(Math.round(center.x - size / 2), 0, imageWidth - size);
  const y = clamp(Math.round(center.y - size / 2), 0, imageHeight - size);
  return { x, y, width: size, height: size };
}

/** Clamp a normalized selection so it never extends past the image edges. Guards
 *  the far-edge rounding case (e.g. x=3839,w=2 on a 3840-wide image) that the
 *  backend bounds validator would otherwise reject as a silent no-op. */
export function clampSelectionToBounds(
  selection: Rect,
  imageWidth: number,
  imageHeight: number
): Rect {
  const x = clamp(selection.x, 0, imageWidth);
  const y = clamp(selection.y, 0, imageHeight);
  return {
    x,
    y,
    width: clamp(selection.width, 0, imageWidth - x),
    height: clamp(selection.height, 0, imageHeight - y),
  };
}
