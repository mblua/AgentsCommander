// @vitest-environment jsdom
//
// #2236 phase 5 (D9a) — the veto proved against the handler TerminalView
// actually passes to attachCustomKeyEventHandler. Assertions 1, 2 and 5 are
// production-bound; 3 and 4 are a declared simulation of propagation.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  registerCompactHostForTests,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../../shared/testing/session-selection";
import { registerShortcuts, unregisterShortcuts } from "../../shared/shortcuts";
import * as compact from "../../shared/sidebar-compact";

type KeyHandler = (event: KeyboardEvent) => boolean;
const xterm = vi.hoisted(() => ({ handlers: [] as KeyHandler[], selection: false }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    loadAddon(): void {}
    open(element: HTMLElement): void { this.element = element; }
    focus(): void {}
    dispose(): void {}
    write(): void {}
    reset(): void {}
    scrollToBottom(): void {}
    paste(): void {}
    resize(): void {}
    hasSelection(): boolean { return xterm.selection; }
    getSelection(): string { return "copied"; }
    attachCustomKeyEventHandler(handler: KeyHandler): void { xterm.handlers.push(handler); }
    onData(): { dispose: () => void } { return { dispose: () => {} }; }
    onResize(): { dispose: () => void } { return { dispose: () => {} }; }
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    activate(): void {}
    fit(): void {}
    proposeDimensions(): { cols: number; rows: number } { return { cols: 80, rows: 24 }; }
  },
}));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class { onContextLoss = vi.fn(); dispose = vi.fn(); },
}));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("../../shared/platform", () => ({ isTauri: true, isBrowser: false }));

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}
const hotkey = () => key({ key: "E", code: "KeyE", ctrlKey: true, shiftKey: true });

describe("TerminalView hotkey handler (#2236 D9a)", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanupRender: (() => void) | null = null;

  beforeEach(async () => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.handlers.length = 0;
    xterm.selection = false;
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_active_session", liveSelection(SESSION_A));
    fake.onInvoke("list_sessions", () => [session({ id: SESSION_A })]);
    fake.resolve("pty_resize", undefined);
    fake.onInvoke("activate_terminal_output", () => ({
      sessionId: SESSION_A, data: [], rows: 24, cols: 80, sequence: 0,
    }));
    fake.resolve("detach_terminal_output", undefined);
    cleanupRender = renderWithFakeTransport(() => <TerminalApp embedded />, fake).cleanup;
    await waitFor(() => expect(xterm.handlers.length).toBeGreaterThan(0));
  });

  afterEach(async () => {
    cleanupRender?.();
    document.body.innerHTML = "";
    await Promise.resolve(); // let the sessions-store MutationObserver drain before teardown
    cleanupDom?.();
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  const handler = (): KeyHandler => xterm.handlers[xterm.handlers.length - 1];

  it("1. consumes the configured hotkey and passes an unrelated key", () => {
    expect(handler()(hotkey())).toBe(false);
    expect(handler()(key({ key: "a", code: "KeyA" }))).toBe(true);
  });

  it("2. does not stop propagation, unlike the copy branch", () => {
    const event = hotkey();
    const stop = vi.spyOn(event, "stopPropagation");
    handler()(event);
    expect(stop).not.toHaveBeenCalled();

    xterm.selection = true;
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn(() => Promise.resolve()) },
    });
    const copy = key({ key: "C", code: "KeyC", ctrlKey: true, shiftKey: true });
    const copyStop = vi.spyOn(copy, "stopPropagation");
    handler()(copy);
    expect(copyStop).toHaveBeenCalled();
  });

  function dispatchThrough(listener: KeyHandler): number {
    const release = registerCompactHostForTests();
    const toggle = vi.spyOn(compact, "toggleSidebarCompact");
    const shortcuts = registerShortcuts();
    const container = document.createElement("div");
    document.body.append(container);
    container.addEventListener("keydown", (e) => { listener(e); });
    const before = compact.sidebarCompact();
    container.dispatchEvent(hotkey());
    const flips = compact.sidebarCompact() === before ? 0 : 1;
    container.remove();
    unregisterShortcuts(shortcuts);
    release();
    expect(toggle.mock.calls.length).toBe(flips);
    return flips;
  }

  it("3. simulation: a false return without stopPropagation toggles exactly once", () => {
    expect(dispatchThrough(handler())).toBe(1);
  });

  it("4. negative control: a variant that stops propagation toggles zero times", () => {
    expect(dispatchThrough((e) => { e.stopPropagation(); return false; })).toBe(0);
  });

  it("5. cross-domain: Dvorak KeyI/'c' on Ctrl+Shift+I falls through to copy", () => {
    compact.setSidebarCompactHotkey("Ctrl+Shift+I");
    xterm.selection = true;
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn(() => Promise.resolve()) },
    });
    const event = key({ key: "c", code: "KeyI", ctrlKey: true, shiftKey: true });
    const stop = vi.spyOn(event, "stopPropagation");
    expect(handler()(event)).toBe(false);
    expect(stop).toHaveBeenCalled();
  });
});
