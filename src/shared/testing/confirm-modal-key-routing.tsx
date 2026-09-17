// @vitest-environment jsdom
//
// Shared key-routing suite for the confirm modals. QuitConfirmModal and
// TaskCleanConfirmModal route Escape, Enter and Tab identically apart from
// their button/callback pair, so the assertions live here once. Each modal
// keeps its own suite and fails on its own regression.
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";
import { render } from "solid-js/web";
import type { JSX } from "solid-js";

export interface ConfirmModalKeyRoutingSpies {
  onCancel: Mock<() => void>;
  onConfirm: Mock<() => void>;
}

export function describeConfirmModalKeyRouting(
  name: string,
  mountModal: (spies: ConfirmModalKeyRoutingSpies) => JSX.Element
): void {
  let cleanup: (() => void) | null = null;

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  function mount(): ConfirmModalKeyRoutingSpies & { buttons: HTMLButtonElement[] } {
    const onCancel = vi.fn<() => void>();
    const onConfirm = vi.fn<() => void>();
    const root = document.createElement("div");
    document.body.appendChild(root);
    cleanup = render(() => mountModal({ onCancel, onConfirm }), root);
    const buttons = Array.from(document.querySelectorAll("button"));
    return { onCancel, onConfirm, buttons };
  }

  function keydown(key: string, shiftKey = false): KeyboardEvent {
    const e = new KeyboardEvent("keydown", {
      key,
      shiftKey,
      cancelable: true,
      bubbles: true,
    });
    document.dispatchEvent(e);
    return e;
  }

  describe(`${name} key routing`, () => {
    it("Escape cancels", () => {
      const { onCancel, onConfirm } = mount();
      const e = keydown("Escape");
      expect(e.defaultPrevented).toBe(true);
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(onConfirm).not.toHaveBeenCalled();
    });

    it("Enter on the destructive button confirms", () => {
      const { onCancel, onConfirm, buttons } = mount();
      buttons[1].focus();
      const e = keydown("Enter");
      expect(e.defaultPrevented).toBe(true);
      expect(onConfirm).toHaveBeenCalledTimes(1);
      expect(onCancel).not.toHaveBeenCalled();
    });

    it("Enter anywhere else cancels", () => {
      const { onCancel, onConfirm, buttons } = mount();
      buttons[0].focus();
      keydown("Enter");
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(onConfirm).not.toHaveBeenCalled();
    });

    it("Tab from the last button wraps to the first", () => {
      const { buttons } = mount();
      buttons[1].focus();
      const e = keydown("Tab");
      expect(e.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(buttons[0]);
    });

    it("Shift+Tab from the first button wraps to the last", () => {
      const { buttons } = mount();
      buttons[0].focus();
      const e = keydown("Tab", true);
      expect(e.defaultPrevented).toBe(true);
      expect(document.activeElement).toBe(buttons[1]);
    });
  });
}
