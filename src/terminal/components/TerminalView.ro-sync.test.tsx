// @vitest-environment jsdom
//
// #1461 — re-selection viewport sync is event-driven: the shared ResizeObserver
// observes every entry container, and the container's content-box change (the
// `hidden` toggle on re-selection) is what drives `syncViewport`, replacing the
// double-rAF heuristic. These tests pin: (a) a re-selected entry's box change
// drives exactly one fit+resize IPC, (b) no sync fires for hidden or destroyed
// entries, and (c) — in terminal-session-registry.test.ts — the observer
// lifecycle follows the registry's exact-once teardown across LRU evictions.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { terminalStore } from "../stores/terminal";
import {
  baseSettings,
  installBrowserDomStubs,
  installDeterministicAnimationFrames,
  MAX_ANIMATION_FRAME_PASSES,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { DeterministicAnimationFrames } from "../../shared/testing/ui-harness";
import { liveSelection, SESSION_A, SESSION_B } from "../../shared/testing/session-selection";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  resizes: { cols: number; rows: number }[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

/** What the tile currently fits to. Mutable: a box change is simulated by
 *  moving it between the attach and the RO delivery. */
const fitViewport = vi.hoisted(() => ({ cols: 74, rows: 23 }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols: number;
    rows: number;
    element: HTMLElement | null = null;
    writes: unknown[] = [];
    resizes: { cols: number; rows: number }[] = [];
    private resizeHandlers = new Set<(size: { cols: number; rows: number }) => void>();

    constructor(options?: { cols?: number; rows?: number }) {
      this.cols = options?.cols ?? 80;
      this.rows = options?.rows ?? 24;
      xterm.instances.push(this);
    }

    loadAddon(addon?: { activate?: (terminal: FakeTerminalInstance) => void }): void {
      addon?.activate?.(this);
    }

    open(element: HTMLElement): void {
      this.element = element;
    }

    focus(): void {}
    dispose(): void {
      this.resizeHandlers.clear();
    }
    write(_data: unknown, callback?: () => void): void {
      callback?.();
    }
    reset(): void {}
    scrollToBottom(): void {}
    paste(): void {}
    hasSelection(): boolean {
      return false;
    }
    getSelection(): string {
      return "";
    }
    attachCustomKeyEventHandler(): void {}
    onData(): { dispose: () => void } {
      return { dispose: () => {} };
    }

    onResize(handler: (size: { cols: number; rows: number }) => void): { dispose: () => void } {
      this.resizeHandlers.add(handler);
      return { dispose: () => this.resizeHandlers.delete(handler) };
    }

    emitResize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
      this.resizes.push({ cols, rows });
      for (const handler of Array.from(this.resizeHandlers)) {
        handler({ cols, rows });
      }
    }

    resize(cols: number, rows: number): void {
      this.emitResize(cols, rows);
    }
  },
}));

const fitCalls = vi.hoisted(() => ({ count: 0 }));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;

    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }

    proposeDimensions(): { cols: number; rows: number } {
      return { cols: fitViewport.cols, rows: fitViewport.rows };
    }

    fit = vi.fn(() => {
      fitCalls.count += 1;
      const terminal = this.terminal;
      if (!terminal) return;
      if (terminal.cols === fitViewport.cols && terminal.rows === fitViewport.rows) {
        return;
      }
      terminal.resize(fitViewport.cols, fitViewport.rows);
    });
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    onContextLoss = vi.fn();
    dispose = vi.fn();
  },
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

vi.mock("../../shared/platform", () => ({
  isTauri: true,
  isBrowser: false,
}));

const ON_SCREEN = SESSION_A;
const SPAWNED = SESSION_B;

/** A drivable ResizeObserver, installed BEFORE `installBrowserDomStubs` so the
 *  harness preserves it (its `previousResizeObserver ?? Noop` fallback keeps a
 *  pre-installed implementation). */
class FakeResizeObserver implements ResizeObserver {
  static instances: FakeResizeObserver[] = [];
  observed: Element[] = [];
  private callback: ResizeObserverCallback;

  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    FakeResizeObserver.instances.push(this);
  }

  observe(target: Element): void {
    this.observed.push(target);
  }

  unobserve(target: Element): void {
    this.observed = this.observed.filter((observed) => observed !== target);
  }

  disconnect(): void {
    this.observed = [];
  }

  fire(target: Element): void {
    const entry = {
      target,
      contentRect: new DOMRectReadOnly(0, 0, 100, 30),
      borderBoxSize: [],
      contentBoxSize: [],
      devicePixelContentBoxSize: [],
      targetRect: new DOMRectReadOnly(0, 0, 100, 30),
    } as unknown as ResizeObserverEntry;
    this.callback([entry], this);
  }
}

function setupTerminalTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", liveSelection(ON_SCREEN));
  fake.onInvoke("list_sessions", () => [session({ id: ON_SCREEN }), session({ id: SPAWNED })]);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.onInvoke("activate_terminal_output", (args) => {
    const sessionId = String(args.sessionId);
    const instance = xterm.instances.find(
      (candidate) => candidate.element?.getAttribute("data-ac-session-id") === sessionId,
    );
    return { sessionId, data: [], rows: instance?.rows ?? 24, cols: instance?.cols ?? 80, sequence: 0 };
  });
  fake.resolve("detach_terminal_output", undefined);
}

function resizesFor(fake: FakeTransport, sessionId: string) {
  return fake.callsFor("pty_resize").filter((call) => call.args.sessionId === sessionId);
}

function containerFor(sessionId: string): HTMLElement {
  const container = document.querySelector<HTMLElement>(`[data-ac-session-id="${sessionId}"]`);
  if (!container) {
    throw new Error(`no terminal container for ${sessionId}`);
  }
  return container;
}

/** Advances frames until `condition` holds, then keeps flushing until the
 *  resize count is stable across two consecutive frames — the attach chain's
 *  awaited pre-seed resize and the settle land one or two frames after the
 *  first resize appears, so snapshots taken right at `condition` would race
 *  them. */
async function settleFrames(
  frames: DeterministicAnimationFrames,
  what: string,
  condition: () => boolean,
  resizeCount: () => number
): Promise<void> {
  for (let pass = 0; pass < MAX_ANIMATION_FRAME_PASSES; pass += 1) {
    if (!condition()) {
      await frames.flushFrame();
      continue;
    }
    const before = resizeCount();
    await frames.flushFrame();
    await frames.flushFrame();
    if (resizeCount() === before) {
      return;
    }
  }
  throw new Error(`settleFrames: ${what} did not settle within ${MAX_ANIMATION_FRAME_PASSES} frames`);
}

describe("TerminalView RO-driven re-selection sync (#1461)", () => {
  let cleanupDom: (() => void) | null = null;
  let frames!: DeterministicAnimationFrames;

  beforeEach(() => {
    FakeResizeObserver.instances.length = 0;
    // BEFORE the browser stubs: the harness preserves a pre-installed RO.
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver;
    cleanupDom = installBrowserDomStubs();
    frames = installDeterministicAnimationFrames();
    resetUiStoresForTests();
    xterm.instances.length = 0;
    fitViewport.cols = 74;
    fitViewport.rows = 23;
    fitCalls.count = 0;
  });

  afterEach(() => {
    frames?.restore();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    terminalStore.setActiveSessionForTests(null);
    vi.restoreAllMocks();
  });

  it("observes each entry container at creation, including the one switched to", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const ro = FakeResizeObserver.instances[FakeResizeObserver.instances.length - 1]!;
      expect(ro.observed).toContain(containerFor(ON_SCREEN));

      terminalStore.setActiveSessionForTests(SPAWNED);
      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      expect(ro.observed).toContain(containerFor(SPAWNED));
    } finally {
      rendered.cleanup();
    }
  });

  it("(a) a re-selected entry's box change drives exactly one fit+resize IPC", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleFrames(
        frames,
        "initial attach",
        () => resizesFor(fake, ON_SCREEN).length > 0,
        () => fake.callsFor("pty_resize").length
      );

      // switch to the second session: its container un-hides (the box change)
      terminalStore.setActiveSessionForTests(SPAWNED);
      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      await settleFrames(
        frames,
        "B attach",
        () => resizesFor(fake, SPAWNED).length > 0,
        () => resizesFor(fake, SPAWNED).length
      );
      const ro = FakeResizeObserver.instances[FakeResizeObserver.instances.length - 1]!;
      expect(ro.observed).toContain(containerFor(SPAWNED));

      // a genuine box change: the fitted grid differs from what the PTY holds
      fitViewport.cols = 75;
      fitViewport.rows = 24;
      const before = resizesFor(fake, SPAWNED).length;
      const fitBefore = fitCalls.count;
      ro.fire(containerFor(SPAWNED));
      await settleFrames(
        frames,
        "RO-driven sync",
        () => resizesFor(fake, SPAWNED).length > before,
        () => resizesFor(fake, SPAWNED).length
      );

      // exactly ONE new resize from the single RO delivery, fitted to the new grid
      expect(resizesFor(fake, SPAWNED).length).toBe(before + 1);
      expect(fitCalls.count).toBeGreaterThan(fitBefore);
      const last = resizesFor(fake, SPAWNED)[resizesFor(fake, SPAWNED).length - 1]!;
      expect(last.args.cols).toBe(75);
      expect(last.args.rows).toBe(24);
    } finally {
      rendered.cleanup();
    }
  });

  it("(b) no sync fires for hidden or destroyed entries", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleFrames(
        frames,
        "initial attach",
        () => resizesFor(fake, ON_SCREEN).length > 0,
        () => fake.callsFor("pty_resize").length
      );

      terminalStore.setActiveSessionForTests(SPAWNED);
      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      await settleFrames(
        frames,
        "B attach",
        () => resizesFor(fake, SPAWNED).length > 0,
        () => resizesFor(fake, SPAWNED).length
      );
      const ro = FakeResizeObserver.instances[FakeResizeObserver.instances.length - 1]!;

      // hidden container (A after the switch): the guard skips it
      const aContainer = containerFor(ON_SCREEN);
      expect(aContainer.hidden).toBe(true);
      const resizesBeforeHidden = fake.callsFor("pty_resize").length;
      ro.fire(aContainer);
      await frames.flush();
      expect(fake.callsFor("pty_resize").length).toBe(resizesBeforeHidden);

      // destroyed entry: the session-destroyed event tears the entry down and
      // unobserves its container; a late RO delivery for it must be a no-op
      const bContainer = containerFor(SPAWNED);
      fake.emitFromBackend("session_destroyed", { id: SPAWNED });
      await waitFor(() => expect(ro.observed).not.toContain(bContainer));
      const resizesBeforeDestroyed = fake.callsFor("pty_resize").length;
      ro.fire(bContainer);
      await frames.flush();
      expect(fake.callsFor("pty_resize").length).toBe(resizesBeforeDestroyed);
    } finally {
      rendered.cleanup();
    }
  });
});
