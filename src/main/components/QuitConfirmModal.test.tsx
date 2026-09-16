// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import QuitConfirmModal from "./QuitConfirmModal";

let cleanup: (() => void) | null = null;

afterEach(() => {
  cleanup?.();
  cleanup = null;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function mount() {
  const onCancel = vi.fn();
  const onQuit = vi.fn();
  const root = document.createElement("div");
  document.body.appendChild(root);
  cleanup = render(
    () => <QuitConfirmModal detachedCount={1} onCancel={onCancel} onQuit={onQuit} />,
    root
  );
  const buttons = Array.from(document.querySelectorAll("button"));
  return { onCancel, onQuit, buttons };
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

describe("QuitConfirmModal key routing", () => {
  it("Escape cancels", () => {
    const { onCancel, onQuit } = mount();
    const e = keydown("Escape");
    expect(e.defaultPrevented).toBe(true);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onQuit).not.toHaveBeenCalled();
  });

  it("Enter on the quit button quits", () => {
    const { onCancel, onQuit, buttons } = mount();
    buttons[1].focus();
    const e = keydown("Enter");
    expect(e.defaultPrevented).toBe(true);
    expect(onQuit).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Enter anywhere else cancels", () => {
    const { onCancel, onQuit, buttons } = mount();
    buttons[0].focus();
    keydown("Enter");
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onQuit).not.toHaveBeenCalled();
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
