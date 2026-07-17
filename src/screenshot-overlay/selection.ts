
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

export interface ClientRectLike {
  left: number;
  top: number;
  width: number;
  height: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export function normalizeSelection(start: Point, end: Point): Rect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

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
