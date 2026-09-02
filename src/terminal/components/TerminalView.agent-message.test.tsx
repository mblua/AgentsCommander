// @vitest-environment jsdom
//
// #1682 — the last coding-agent message timestamp in the terminal status strip.
//
// The user asked for "el último timestamp de sistema del último mensaje recibido
// desde el coding agent por esta terminal", rendered to the LEFT of the existing
// COLS/ROWS chip and persisted. The backend half (phases 01-04) stamps the
// busy->idle edge of an armed session into the replica's config and emits
// `session_agent_message`; this file pins the renderer half:
//
//   - nothing renders at all when there is no recorded value,
//   - the strip is one flex row and the stamp is the chip's PREVIOUS sibling,
//     which is also what proves the chip was re-parented rather than left on
//     `container` by a surviving `container.appendChild(gridStatus)`,
//   - hydration and the live event are reconciled by a MONOTONIC watermark, so a
//     slow hydration cannot drag the rendering backwards,
//   - a rejected hydration (browser mode has no dispatcher entry for the
//     command) degrades to a blank chip and never breaks the terminal.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { terminalStore } from "../stores/terminal";
import { liveSelection, SESSION_A, SESSION_B } from "../../shared/testing/session-selection";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: number[][];
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols: number;
    rows: number;
    element: HTMLElement | null = null;
    writes: number[][] = [];

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
    dispose(): void {}
    write(data: unknown, callback?: () => void): void {
      this.writes.push(Array.from(data as Uint8Array));
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
    onResize(): { dispose: () => void } {
      return { dispose: () => {} };
    }
    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    activate(): void {}
    // The tile is already at the terminal's size, so nothing reflows and the
    // resize path stays out of the way of what this file measures.
    proposeDimensions(): { cols: number; rows: number } {
      return { cols: 80, rows: 24 };
    }
    fit = vi.fn(() => {});
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
const OTHER = SESSION_B;

/** Shaped like `setupTerminalTransport` in `TerminalView.spawn-size.test.tsx:178`.
 *  That one is module-local there too, so this is a local equivalent rather than
 *  an import: exporting it would add an eighth file to this phase. */
function setupTerminalTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", liveSelection(ON_SCREEN));
  fake.onInvoke("list_sessions", () => [
    session({ id: ON_SCREEN }),
    session({ id: OTHER }),
  ]);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.onInvoke("activate_terminal_output", (args) => ({
    sessionId: String(args.sessionId),
    data: [],
    rows: 24,
    cols: 80,
    sequence: 0,
  }));
  fake.resolve("detach_terminal_output", undefined);
}

/** Shaped like `emitPtyOutput` in `TerminalView.saturation.test.tsx:185` — a
 *  module-local `function` closing over a module-local `currentFake`, for the
 *  same reason. */
let currentFake: FakeTransport | null = null;
function emitPtyOutput(sessionId: string, sequence: number, data: number[]): void {
  currentFake?.emitFromBackend("pty_output", { sessionId, data, sequence });
}

function stampFor(sessionId: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(
    `[data-testid="terminal-agent-message-${sessionId}"]`
  );
  if (!el) throw new Error(`no agent-message stamp for ${sessionId}`);
  return el;
}

function gridFor(sessionId: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(
    `[data-testid="terminal-grid-size-${sessionId}"]`
  );
  if (!el) throw new Error(`no grid chip for ${sessionId}`);
  return el;
}

/** An instant built from LOCAL parts, so every assertion pins the FORMAT and not
 *  the runner's timezone. */
function localIso(
  month: number,
  day: number,
  hours: number,
  minutes: number
): string {
  return new Date(2026, month - 1, day, hours, minutes).toISOString();
}

/** Lets the promise chains behind the hydration settle before an assertion, so a
 *  stale resolution gets its chance to write and be caught doing it. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** `emitFromBackend` SILENTLY RETURNS when nothing is listening
 *  (`fake-transport.ts:120-122`), so every emit below waits for the registration
 *  first: without this an assertion could pass against a listener that was never
 *  registered at all. */
async function awaitAgentMessageListener(fake: FakeTransport): Promise<void> {
  await waitFor(() =>
    expect(fake.listens.some((l) => l.event === "session_agent_message")).toBe(true)
  );
}

describe("TerminalView last coding-agent message stamp (#1682)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
    currentFake = null;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    terminalStore.setActiveSessionForTests(null);
    currentFake = null;
    vi.restoreAllMocks();
  });

  it("renders nothing at all when there is no recorded value", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", null);
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("get_last_agent_message").length).toBeGreaterThan(0)
      );
      await settle();

      const stamp = stampFor(ON_SCREEN);
      expect(stamp.textContent).toBe("");
      expect(stamp.hidden).toBe(true);
      // Not an empty box: hidden, and with no inline `display` to beat the
      // user-agent `[hidden] { display: none }` rule.
      expect(stamp.style.display).toBe("");

      // The COLS/ROWS chip is untouched by this phase.
      const grid = gridFor(ON_SCREEN);
      expect(grid.hidden).toBe(false);
      expect(grid.getAttribute("aria-label")).toBe("Terminal grid size");
      expect(grid.className).toBe("terminal-grid-status");
      expect(grid.textContent).toMatch(/^COLS: \d+ ROWS: \d+$/);
    } finally {
      rendered.cleanup();
    }
  });

  it("hydrates the persisted value and renders it left of the grid chip", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", localIso(8, 31, 21, 29));
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29"));

      const stamp = stampFor(ON_SCREEN);
      const grid = gridFor(ON_SCREEN);
      expect(stamp.hidden).toBe(false);

      // D2: the chip lives in the strip, not on `container`. A surviving
      // `container.appendChild(gridStatus)` would have pulled it back out and
      // this is where that shows.
      expect(grid.parentElement).toBe(stamp.parentElement);
      expect(grid.parentElement?.className).toBe("terminal-status-strip");
      expect(grid.previousElementSibling).toBe(stamp);
    } finally {
      rendered.cleanup();
    }
  });

  it("updates live from session_agent_message without a remount or a second invoke", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", localIso(8, 31, 21, 29));
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29"));
      const stamp = stampFor(ON_SCREEN);
      const instancesAfterHydration = xterm.instances.length;
      const invokesAfterHydration = fake.callsFor("get_last_agent_message").length;

      await awaitAgentMessageListener(fake);
      fake.emitFromBackend("session_agent_message", {
        sessionId: ON_SCREEN,
        at: localIso(9, 1, 7, 5),
      });
      await settle();

      expect(stampFor(ON_SCREEN)).toBe(stamp);
      expect(stamp.textContent).toBe("09-01 07:05");
      expect(xterm.instances).toHaveLength(instancesAfterHydration);
      expect(fake.callsFor("get_last_agent_message")).toHaveLength(invokesAfterHydration);
    } finally {
      rendered.cleanup();
    }
  });

  it("ignores an event for a different session and applies one for the visible session", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", localIso(8, 31, 21, 29));
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29"));
      await awaitAgentMessageListener(fake);

      fake.emitFromBackend("session_agent_message", {
        sessionId: OTHER,
        at: localIso(12, 25, 23, 59),
      });
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29");

      // The same emit for the VISIBLE session does land, so the assertion above
      // cannot pass on a listener that was never registered.
      fake.emitFromBackend("session_agent_message", {
        sessionId: ON_SCREEN,
        at: localIso(12, 25, 23, 59),
      });
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("12-25 23:59");
    } finally {
      rendered.cleanup();
    }
  });

  it("never moves the rendering backwards: an older event is dropped (D4 case 1)", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", null);
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await awaitAgentMessageListener(fake);

      fake.emitFromBackend("session_agent_message", {
        sessionId: ON_SCREEN,
        at: localIso(9, 1, 7, 5),
      });
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("09-01 07:05");

      fake.emitFromBackend("session_agent_message", {
        sessionId: ON_SCREEN,
        at: localIso(8, 31, 21, 29),
      });
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("09-01 07:05");
    } finally {
      rendered.cleanup();
    }
  });

  it("never moves the rendering backwards: a late hydration is dropped (D4 case 2)", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);

    // `fake.resolve` is only `onInvoke(cmd, () => value)` (`fake-transport.ts:52-54`)
    // and cannot hold a call open, so the deferred is registered directly.
    let resolveHydration: (value: string | null) => void = () => {};
    const hydration = new Promise<string | null>((resolve) => {
      resolveHydration = resolve;
    });
    fake.onInvoke("get_last_agent_message", () => hydration);
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await awaitAgentMessageListener(fake);

      // The newer event lands while the hydration is still in flight...
      fake.emitFromBackend("session_agent_message", {
        sessionId: ON_SCREEN,
        at: localIso(9, 1, 7, 5),
      });
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("09-01 07:05");

      // ...and the hydration then resolves to something OLDER. This is the case
      // an entry-identity guard alone would not catch: both results belong to
      // the same entry, so only the watermark can drop it.
      resolveHydration(localIso(8, 31, 21, 29));
      await settle();
      expect(stampFor(ON_SCREEN).textContent).toBe("09-01 07:05");
    } finally {
      rendered.cleanup();
    }
  });

  it("degrades to a blank chip when the command rejects, and still renders the terminal", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.reject("get_last_agent_message", "no dispatcher entry");
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("get_last_agent_message").length).toBeGreaterThan(0)
      );
      await settle();

      const stamp = stampFor(ON_SCREEN);
      expect(stamp.textContent).toBe("");
      expect(stamp.hidden).toBe(true);
      expect(gridFor(ON_SCREEN).textContent).toMatch(/^COLS: \d+ ROWS: \d+$/);
    } finally {
      rendered.cleanup();
    }
  });

  it("leaves the stamp byte-identical across a pty_output burst", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    fake.resolve("get_last_agent_message", localIso(8, 31, 21, 29));
    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29"));
      const before = stampFor(ON_SCREEN).textContent;
      const writesBefore = xterm.instances[0].writes.length;

      for (let sequence = 1; sequence <= 12; sequence += 1) {
        emitPtyOutput(ON_SCREEN, sequence, [65, 66, 67]);
      }
      await settle();

      // The burst really was admitted, so the pin is about the stamp and not
      // about output that never arrived.
      expect(xterm.instances[0].writes.length).toBeGreaterThan(writesBefore);
      expect(stampFor(ON_SCREEN).textContent).toBe(before);
      expect(stampFor(ON_SCREEN).textContent).toBe("08-31 21:29");
    } finally {
      rendered.cleanup();
    }
  });
});
