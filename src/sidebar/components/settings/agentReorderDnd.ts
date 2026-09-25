/** #2543 - pure drag-reorder arithmetic for the Settings coding-agent list.
 *  A leaf: imports nothing, owns no DOM. */

export const DRAG_THRESHOLD = 4; // px of pointer travel before a drag starts
export const EDGE = 36; // px from a list edge where auto-scroll begins
export const MAX_SPEED = 14; // px per frame at the very edge

/** Count of the non-dragged rows (visual order) whose midpoint is above pointerY. */
export function insertionSlot(pointerY: number, otherRowRects: readonly DOMRect[]): number {
  const i = otherRowRects.findIndex((rect) => pointerY < rect.top + rect.height / 2);
  return i === -1 ? otherRowRects.length : i;
}

/** The slot is counted without the dragged row and the backend removes it
 *  before inserting, so the absolute target index is the slot itself. */
export function reorderIndex(_fromIndex: number, slot: number): number {
  return slot;
}

export function autoScrollDelta(pointerY: number, listTop: number, listBottom: number): number {
  if (pointerY < listTop + EDGE) {
    return -Math.min(1, (listTop + EDGE - pointerY) / EDGE) * MAX_SPEED;
  }
  if (pointerY > listBottom - EDGE) {
    return Math.min(1, (pointerY - (listBottom - EDGE)) / EDGE) * MAX_SPEED;
  }
  return 0;
}

/** Remove at fromIndex, then insert at targetIndex, on a copy (p1's transition). */
export function reorderedIds(ids: readonly string[], fromIndex: number, targetIndex: number): string[] {
  const next = [...ids];
  const [moved] = next.splice(fromIndex, 1);
  next.splice(targetIndex, 0, moved);
  return next;
}
