// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { trapTabFocus } from "./focus-trap";

function buttons(count: number, focusIndex: number | null = null): HTMLButtonElement[] {
  const els = Array.from({ length: count }, (_, index) => {
    const button = document.createElement("button");
    button.textContent = `button-${index}`;
    document.body.appendChild(button);
    return button;
  });
  if (focusIndex !== null) els[focusIndex].focus();
  return els;
}

const tab = (shiftKey = false): KeyboardEvent =>
  new KeyboardEvent("keydown", { key: "Tab", shiftKey, cancelable: true });

afterEach(() => document.body.replaceChildren());

describe("trapTabFocus", () => {
  it("no-op with fewer than two focusables", () => {
    const [only] = buttons(1, 0);
    const e = tab();
    trapTabFocus(e, [only]);
    expect(e.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(only);

    const empty = tab();
    trapTabFocus(empty, []);
    expect(empty.defaultPrevented).toBe(false);
  });

  it("moves focus to the first focusable when the active element is outside the set", () => {
    const els = buttons(2);
    document.body.focus();
    const e = tab();
    trapTabFocus(e, els);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(els[0]);
  });

  it("moves focus to the last focusable on Shift+Tab when the active element is outside the set", () => {
    const els = buttons(2);
    document.body.focus();
    const e = tab(true);
    trapTabFocus(e, els);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(els[1]);
  });

  it("wraps Tab on the last focusable back to the first", () => {
    const els = buttons(3, 2);
    const e = tab();
    trapTabFocus(e, els);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(els[0]);
  });

  it("wraps Shift+Tab on the first focusable back to the last", () => {
    const els = buttons(3, 0);
    const e = tab(true);
    trapTabFocus(e, els);
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(els[2]);
  });

  it("leaves mid-list Tab and Shift+Tab untouched", () => {
    const els = buttons(3, 1);

    const forward = tab();
    trapTabFocus(forward, els);
    expect(forward.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(els[1]);

    const backward = tab(true);
    trapTabFocus(backward, els);
    expect(backward.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(els[1]);
  });
});
