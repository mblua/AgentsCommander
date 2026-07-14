// @vitest-environment jsdom
//
// #973 — a coding agent's tile stays blank on spawn.
//
// AC opened every ConPTY at a hardcoded 120x30 and let the view correct it a few
// hundred milliseconds later. That correction lands inside the agent's TUI
// startup: the child redraws its still-empty viewport, loses the wakeup for the
// content that becomes ready right after, and the tile stays blank while the
// process runs on. dev-rust measured it on a bare ConPTY — 8/10 blank with the
// resize burst, 0/10 without — and on the user's production logs (p = 0.0103).
//
// The frontend half is this: hand `create_session` the size the view has ALREADY
// fitted to, and then do not resize the PTY to a size it is already at. Both
// halves are load-bearing, and dev-rust measured why:
//
//   R8  open at 74x23, redundant same-size burst still fires ..... 0/10 blank
//   R9  open at 74x23, the fit lands on 74x24 — one row of drift .. 6/10 blank
//
// So the size handed over at create time must be BYTE-EXACT with the size the
// post-mount fit produces. These tests pin that, and pin the failure modes that
// would silently un-fix it: a resize that still fires for nothing, a terminal
// that starts at xterm's 80x24 default and reflows, and a failed resize that
// poisons the dedup so the retry is skipped and the PTY is wedged forever.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { SessionAPI } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { resetPtyViewportForTests } from "../../shared/terminal-viewport";
import { terminalStore } from "../stores/terminal";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  resizes: { cols: number; rows: number }[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

/** What the tile currently fits to. Mutable: drift is simulated by moving it
 *  between the create and the attach. */
const fitViewport = vi.hoisted(() => ({ cols: 74, rows: 23 }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols: number;
    rows: number;
    element: HTMLElement | null = null;
    writes: unknown[] = [];
    resizes: { cols: number; rows: number }[] = [];
    private resizeHandlers = new Set<(size: { cols: number; rows: number }) => void>();

    // Real xterm honours cols/rows from the constructor and defaults to 80x24.
    // That default is the thing #973 must stop relying on.
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
    write(data: unknown): void {
      this.writes.push(data);
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

    onResize(
      handler: (size: { cols: number; rows: number }) => void
    ): { dispose: () => void } {
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

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;

    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }

    /** The computation `TerminalView` probes at create time — and the one `fit()`
     *  runs at attach time. Same function, which is the entire point. */
    proposeDimensions(): { cols: number; rows: number } {
      return { cols: fitViewport.cols, rows: fitViewport.rows };
    }

    /** Faithful to the real FitAddon: it resizes the terminal ONLY when the
     *  proposal differs from what the terminal already is. A terminal started at
     *  the size the PTY was opened at therefore never reflows and never fires
     *  `onResize` — which is exactly what has to be true for the resize to stop
     *  reaching the child. */
    fit = vi.fn(() => {
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

const ON_SCREEN = "session-on-screen";
const SPAWNED = "session-spawned";

function setupTerminalTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", ON_SCREEN);
  fake.onInvoke("list_sessions", () => [
    session({ id: ON_SCREEN }),
    session({ id: SPAWNED }),
  ]);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("get_screen_snapshot", null);
  fake.resolve("set_last_prompt", undefined);
  fake.resolve("create_session", session({ id: SPAWNED }));
}

function resizesFor(fake: FakeTransport, sessionId: string) {
  return fake
    .callsFor("pty_resize")
    .filter((call) => call.args.sessionId === sessionId);
}

/** The real sequence: a tile is on screen, the sidebar creates a session, and the
 *  view switches to it. Returns the args `create_session` was invoked with. */
async function createWhileOnScreen(fake: FakeTransport) {
  await waitFor(() => expect(xterm.instances).toHaveLength(1));
  await SessionAPI.create({ cwd: "C:\\Project" });
  return fake.lastCall("create_session")!.args;
}

async function attachSpawnedSession() {
  terminalStore.setActiveSession(SPAWNED);
  await waitFor(() => expect(xterm.instances).toHaveLength(2));
  return xterm.instances[1];
}

describe("TerminalView PTY spawn size (#973)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    resetPtyViewportForTests();
    xterm.instances.length = 0;
    fitViewport.cols = 74;
    fitViewport.rows = 23;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    resetPtyViewportForTests();
    terminalStore.setActiveSession(null);
    vi.restoreAllMocks();
  });

  it("opens the PTY at the size the tile on screen is already fitted to", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      const args = await createWhileOnScreen(fake);

      // Not a guess: this is `FitAddon.proposeDimensions()` — the same computation
      // the new terminal's own post-mount fit will run — measured against the tile
      // already on screen in the same host.
      expect(args.cols).toBe(74);
      expect(args.rows).toBe(23);
    } finally {
      rendered.cleanup();
    }
  });

  it("issues no resize at all when the fitted size already equals the spawn size", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await createWhileOnScreen(fake);
      await attachSpawnedSession();

      // Let every resize path have its turn: the double requestAnimationFrame in
      // scheduleViewportSync, the ResizeObserver, and xterm's own onResize.
      await new Promise((resolve) => setTimeout(resolve, 50));

      // The session that was opened at the fitted size is never resized. This is
      // the whole fix: no resize reaches the child while it is starting up.
      expect(resizesFor(fake, SPAWNED)).toHaveLength(0);

      // ...and the resize path is not simply gone. The session already on screen
      // was created with no size (it predates this view), so it still fits and
      // resizes exactly as before.
      expect(resizesFor(fake, ON_SCREEN).length).toBeGreaterThan(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("starts xterm at the size the PTY was opened at, so the fit finds nothing to do", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await createWhileOnScreen(fake);
      const spawned = await attachSpawnedSession();
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Started at 74x23, not at xterm's 80x24 default...
      expect({ cols: spawned.cols, rows: spawned.rows }).toEqual({ cols: 74, rows: 23 });
      // ...so it never reflowed, and never fired onResize.
      expect(spawned.resizes).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("resizes exactly once, and says so, when the fit drifts from the spawn size", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await createWhileOnScreen(fake);

      // The tile measured 74x23 at create time and fits to 74x24 at attach time:
      // one row of drift — a scrollbar, a late font metric, an extra layout pass.
      // dev-rust measured this exact case at 6/10 blank, so it must not pass
      // silently.
      fitViewport.rows = 24;

      await attachSpawnedSession();
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Correctness first: the PTY is told the truth. But exactly once — the
      // burst of identical resizes is gone.
      const resizes = resizesFor(fake, SPAWNED);
      expect(resizes).toHaveLength(1);
      expect(resizes[0].args).toEqual({ sessionId: SPAWNED, cols: 74, rows: 24 });

      // And it is reported, because a silent drift is a silent 6/10 blank.
      expect(
        warn.mock.calls.some((call) => String(call[0]).includes("spawn-size drift"))
      ).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("still fits and resizes a session that was opened without a spawn size", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));

      // No size was ever handed over for this one (startup restore, the delivery
      // loop, a session from a previous run). It must behave exactly as it always
      // has: start at 80x24, fit, and tell the PTY.
      await waitFor(() => expect(resizesFor(fake, ON_SCREEN).length).toBeGreaterThan(0));
      expect(resizesFor(fake, ON_SCREEN)[0].args).toEqual({
        sessionId: ON_SCREEN,
        cols: 74,
        rows: 23,
      });
    } finally {
      rendered.cleanup();
    }
  });

  // The backend's plausibility floor (`PtyViewport::PLAUSIBLE_MIN`, 20x5). Below it,
  // `from_fit` does NOT fail the spawn — it opens the PTY at 120x30 and warns. So a
  // below-floor fit that this side sent AND recorded is the wedge: the PTY is at
  // 120x30, the view believes it is at 2x1, and the resize that would correct it is
  // deduped away as a no-op.
  //
  // And it is not a corner case. The probe is exact by construction — the same
  // `proposeDimensions()`, against the same box — so a container that is collapsed
  // at create time is still collapsed at attach time. The fit finds the terminal
  // already at 2x1, never reflows, never fires onResize, and nothing ever tells the
  // PTY the truth. Refusing to record is what keeps the resize path free.
  it("sends no size for a collapsed tile, and still corrects the PTY afterwards", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake);

    // A laid-out but collapsed box. xterm does not report a zero for this — it
    // clamps to its own MINIMUM_COLS = 2 / MINIMUM_ROWS = 1 — so the fit is a
    // perfectly well-formed 2x1 that a `> 0` check waves straight through.
    fitViewport.cols = 2;
    fitViewport.rows = 1;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      const args = await createWhileOnScreen(fake);

      // Below the floor: not sent, so the backend opens at its own 120x30 default —
      // and, crucially, nothing is recorded on this side.
      expect(args.cols).toBeNull();
      expect(args.rows).toBeNull();

      const spawned = await attachSpawnedSession();
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Nothing recorded, so the terminal starts at xterm's default, fits to the
      // collapsed 2x1, and TELLS the PTY. The corrective resize goes out normally:
      // the resize path has no floor, deliberately (a user really can drag a window
      // down this far), and only the spawn path does.
      expect({ cols: spawned.cols, rows: spawned.rows }).toEqual({ cols: 2, rows: 1 });

      const resizes = resizesFor(fake, SPAWNED);
      expect(resizes.length).toBeGreaterThan(0);
      expect(resizes[0].args).toEqual({ sessionId: SPAWNED, cols: 2, rows: 1 });
    } finally {
      rendered.cleanup();
    }
  });

  // A failed resize rolls `lastSentViewport` back so that a SUBSEQUENT identical
  // resize is not deduped away — but after the dedup, there usually is no subsequent
  // one. The burst collapses to a single call, and when that one is the one that
  // failed, the PTY just stays at the wrong size.
  //
  // So this test gives the failure nothing to hide behind. The terminal is fully
  // settled — no pending rAF, no pending fit, no burst left — and then ONE resize is
  // driven through xterm's onResize, exactly as a user dragging the window would.
  // That one fails. Nothing else in this system will ever call `sendPtyResize` for
  // this session again. A rollback that is not a real retry leaves it at one call.
  it("re-sends a failed resize that is the only one of its burst", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = new FakeTransport();
    setupTerminalTransport(fake);

    let spawnedAttempts = 0;
    fake.onInvoke("pty_resize", (args) => {
      if (args.sessionId !== SPAWNED) {
        return undefined;
      }
      spawnedAttempts += 1;
      if (spawnedAttempts === 1) {
        throw new Error("pty_resize failed");
      }
      return undefined;
    });

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await createWhileOnScreen(fake);
      const spawned = await attachSpawnedSession();
      await new Promise((resolve) => setTimeout(resolve, 50));

      // Opened at the size the tile was already fitted to, so the attach sent
      // nothing at all. There is no burst.
      expect(resizesFor(fake, SPAWNED)).toHaveLength(0);

      // The user drags the window. xterm reflows and fires onResize: one resize,
      // long after every scheduled fit has run. It fails.
      spawned.emitResize(74, 24);

      await waitFor(
        () => expect(resizesFor(fake, SPAWNED).length).toBeGreaterThanOrEqual(2),
        2000
      );

      // The attempt and the re-send both carry the size the terminal is actually at.
      // Nothing but the retry could have produced that second call.
      const resizes = resizesFor(fake, SPAWNED);
      expect(resizes[0].args).toEqual({ sessionId: SPAWNED, cols: 74, rows: 24 });
      expect(resizes[1].args).toEqual({ sessionId: SPAWNED, cols: 74, rows: 24 });
    } finally {
      rendered.cleanup();
    }
  });

  it("gives up loudly, and boundedly, when the resize can never land", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const fake = new FakeTransport();
    setupTerminalTransport(fake);

    fake.onInvoke("pty_resize", (args) => {
      if (args.sessionId !== SPAWNED) {
        return undefined;
      }
      throw new Error("pty_resize failed");
    });

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);

    try {
      await createWhileOnScreen(fake);
      const spawned = await attachSpawnedSession();
      await new Promise((resolve) => setTimeout(resolve, 50));

      spawned.emitResize(74, 24);

      // A PTY stranded at a size the terminal is not must never be mistaken for a
      // PTY that was resized. When the budget runs out, it says so.
      await waitFor(
        () =>
          expect(
            error.mock.calls.some((call) => String(call[0]).includes("giving up"))
          ).toBe(true),
        3000
      );

      // And the budget is a budget: the first attempt plus a fixed number of
      // re-sends, not an endless loop against a backend that is gone.
      expect(resizesFor(fake, SPAWNED).length).toBeLessThanOrEqual(4);
    } finally {
      rendered.cleanup();
    }
  });
});
