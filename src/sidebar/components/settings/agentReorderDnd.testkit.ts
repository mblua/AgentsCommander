import { vi } from "vitest";

/** #2577 - test helpers for agent-list pointer drags (technique of
 *  SettingsModal.automation.test.ts). jsdom has no PointerEvent and no layout:
 *  pointer events are MouseEvents with a defined pointerId. */
export const dispatchPointer = (
  target: Element,
  type: string,
  clientX: number,
  clientY: number,
): MouseEvent => {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX, clientY, button: 0 });
  Object.defineProperty(event, "pointerId", { value: 1 });
  target.dispatchEvent(event);
  return event;
};

/** Gives each row a `rowHeight` px tall box stacked from 0 and stubs pointer
 *  capture on the grip. */
export const stubAgentRowGeometry = (
  rows: readonly HTMLElement[],
  grip: HTMLElement,
  rowHeight = 40,
): void => {
  rows.forEach((row, k) => {
    const top = k * rowHeight;
    row.getBoundingClientRect = () =>
      ({ top, bottom: top + rowHeight, height: rowHeight, left: 0, right: 200, width: 200, x: 0, y: top }) as DOMRect;
    Object.defineProperty(row, "offsetTop", { configurable: true, value: top });
    Object.defineProperty(row, "offsetHeight", { configurable: true, value: rowHeight });
  });
  let held = false;
  grip.setPointerCapture = vi.fn(() => { held = true; });
  grip.releasePointerCapture = vi.fn(() => { held = false; });
  grip.hasPointerCapture = vi.fn(() => held);
};
