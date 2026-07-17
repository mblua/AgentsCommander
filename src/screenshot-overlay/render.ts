
import { magnifierSourceRect, type Point, type Rect } from "./selection";

export const MAGNIFIER_SOURCE_PX = 32;
export const MAGNIFIER_DEST_CSS = 140;
export const MAGNIFIER_OFFSET_CSS = 24;
export const SELECTION_BORDER_CSS = 1.5;

export interface OverlayRenderModel {
  width: number;
  height: number;
  physicalPerCss: number;
}

export function drawOverlay(
  ctx: CanvasRenderingContext2D,
  image: CanvasImageSource,
  model: OverlayRenderModel,
  selection: Rect | null,
  hover: Point | null
): void {
  const { width, height } = model;
  const ratio = model.physicalPerCss > 0 ? model.physicalPerCss : 1;

  ctx.clearRect(0, 0, width, height);
  ctx.drawImage(image, 0, 0, width, height);

  ctx.fillStyle = "rgba(0, 0, 0, 0.45)";
  ctx.fillRect(0, 0, width, height);

  if (selection && selection.width > 0 && selection.height > 0) {
    ctx.drawImage(
      image,
      selection.x,
      selection.y,
      selection.width,
      selection.height,
      selection.x,
      selection.y,
      selection.width,
      selection.height
    );
    ctx.lineWidth = Math.max(1, SELECTION_BORDER_CSS * ratio);
    ctx.strokeStyle = "rgba(120, 200, 255, 0.95)";
    ctx.strokeRect(selection.x, selection.y, selection.width, selection.height);
  }

  if (hover) drawMagnifier(ctx, image, model, hover);
}

function drawMagnifier(
  ctx: CanvasRenderingContext2D,
  image: CanvasImageSource,
  model: OverlayRenderModel,
  center: Point
): void {
  const { width, height } = model;
  const ratio = model.physicalPerCss > 0 ? model.physicalPerCss : 1;
  const dest = MAGNIFIER_DEST_CSS * ratio;
  const radius = dest / 2;
  const offset = MAGNIFIER_OFFSET_CSS * ratio;
  const src = magnifierSourceRect(center, MAGNIFIER_SOURCE_PX, width, height);

  let cx = center.x + offset + radius;
  let cy = center.y + offset + radius;
  if (cx + radius > width) cx = center.x - offset - radius;
  if (cy + radius > height) cy = center.y - offset - radius;
  cx = Math.max(radius, Math.min(width - radius, cx));
  cy = Math.max(radius, Math.min(height - radius, cy));

  ctx.save();
  ctx.beginPath();
  ctx.arc(cx, cy, radius, 0, Math.PI * 2);
  ctx.closePath();
  ctx.clip();
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(
    image,
    src.x,
    src.y,
    src.width,
    src.height,
    cx - radius,
    cy - radius,
    dest,
    dest
  );
  ctx.restore();

  ctx.lineWidth = Math.max(1, 2 * ratio);
  ctx.strokeStyle = "rgba(255, 255, 255, 0.9)";
  ctx.beginPath();
  ctx.arc(cx, cy, radius, 0, Math.PI * 2);
  ctx.stroke();

  const magX = cx - radius + ((center.x - src.x) / src.width) * dest;
  const magY = cy - radius + ((center.y - src.y) / src.height) * dest;
  ctx.strokeStyle = "rgba(255, 80, 80, 0.9)";
  ctx.lineWidth = Math.max(1, ratio);
  ctx.beginPath();
  ctx.moveTo(cx - radius, magY);
  ctx.lineTo(cx + radius, magY);
  ctx.moveTo(magX, cy - radius);
  ctx.lineTo(magX, cy + radius);
  ctx.stroke();
}
