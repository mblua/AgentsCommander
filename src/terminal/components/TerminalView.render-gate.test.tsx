// @vitest-environment jsdom
//
// #955 — a new terminal rendered NOTHING until the `get_screen_snapshot` IPC
// round-trip settled. A Trace capture proved the agent painted at 418 ms and
// kept painting for the entire 5.4 s the user stared at a black tile: every
// byte reached the backend, and the frontend buffered them in an array instead
// of writing them to xterm.
//
// The contract these tests lock down:
//   1. A live PTY byte is NEVER gated behind an IPC round-trip. It renders on
//      arrival, whether the snapshot settles late, or never.
//   2. The snapshot still restores re-attach scrollback — with no duplicated
//      and no missing output — by reconciling AFTER the fact, not by gating.
//
// The xterm double models `reset()` faithfully: `writes` is the full write
// history (proving live bytes were rendered immediately), `screen` is what is
// actually visible now (proving the reconciled result is correct).
//
// #1363 restored this suite. The one thing that moved is where the snapshot
// comes from: it is the value `activate_terminal_output` (the attach) resolves
// to, not a separate `get_screen_snapshot` fetch. The contract above is
// unchanged, and every attach now re-seeds with a reset (plan 3.4.2), so the
// fast path resets once too.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import type { PtyScreenSnapshot } from "../../shared/types";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import {
  dormantSelection,
  initialSelection,
  liveSelection,
  SESSION_A,
} from "../../shared/testing/session-selection";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: unknown[];
  screen: unknown[];
  resets: number;
  disposed: boolean;
  buffer: {
    active: {
      viewportY: number;
      baseY: number;
      length: number;
      type: "normal" | "alternate";
      cursorX: number;
      getLine(index: number): { isWrapped: boolean } | undefined;
    };
  };
  resizes: { cols: number; rows: number }[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
  reset(): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

const fitViewport = vi.hoisted(() => ({
  cols: 88,
  rows: 26,
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    writes: unknown[] = [];
    screen: unknown[] = [];
    resets = 0;
    disposed = false;
    buffer: {
      active: {
        viewportY: number;
        baseY: number;
        length: number;
        type: "normal" | "alternate";
        cursorX: number;
        getLine(index: number): { isWrapped: boolean } | undefined;
      };
    } = {
      active: {
        viewportY: 0,
        baseY: 0,
        length: 0,
        type: "normal",
        cursorX: 0,
        getLine: (index: number) =>
          index >= 0 && index < this.buffer.active.length ? { isWrapped: false } : undefined,
      },
    };
    resizes: { cols: number; rows: number }[] = [];
    private resizeHandlers = new Set<(size: { cols: number; rows: number }) => void>();

    constructor() {
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
      this.disposed = true;
      this.resizeHandlers.clear();
    }

    write(data: unknown, callback?: () => void): void {
      this.assertLive();
      this.writes.push(data);
      this.screen.push(data);
      // The #1489 attach pipeline awaits the optional parse-completion
      // callback (the write fence) and reads the buffer metrics at settle.
      // This fake applies bytes synchronously, so invoking the callback right
      // after the push is faithful to this file's model.
      callback?.();
    }

    /** Real xterm RIS: clears the screen and scrollback. */
    reset(): void {
      this.assertLive();
      this.resets += 1;
      this.screen.length = 0;
    }

    scrollToBottom(): void {}

    scrollToTop(): void {
      this.assertLive();
      this.buffer.active.viewportY = 0;
    }

    scrollToLine(line: number): void {
      this.assertLive();
      this.buffer.active.viewportY = Math.max(0, Math.min(this.buffer.active.baseY, line));
    }

    scrollLines(amount: number): void {
      this.assertLive();
      this.buffer.active.viewportY = Math.max(
        0,
        Math.min(this.buffer.active.baseY, this.buffer.active.viewportY + amount),
      );
    }

    scrollPages(amount: number): void {
      this.assertLive();
      this.buffer.active.viewportY = Math.max(
        0,
        Math.min(this.buffer.active.baseY, this.buffer.active.viewportY + amount * this.rows),
      );
    }

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
      this.assertLive();
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

    private assertLive(): void {
      if (this.disposed) throw new Error("terminal disposed");
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;

    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }

    fit = vi.fn(() => {
      this.terminal?.resize(fitViewport.cols, fitViewport.rows);
    });
    proposeDimensions(): { cols: number; rows: number } {
      return { cols: fitViewport.cols, rows: fitViewport.rows };
    }
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

const SNAP = [83, 78, 65, 80]; // "SNAP"
const LIVE = [76, 73, 86, 69]; // "LIVE"
const NEXT = [78, 69, 88, 84]; // "NEXT"

function setupTerminalTransport(fake: FakeTransport, sessions = [session()]): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve(
    "get_active_session",
    sessions[0] ? liveSelection(sessions[0].id) : initialSelection(),
  );
  fake.onInvoke("list_sessions", () => sessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("activate_terminal_output", null);
  fake.resolve("detach_terminal_output", undefined);
  fake.resolve("set_last_prompt", undefined);
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function onlySession() {
  return session({
    id: SESSION_A,
    name: "wg-1-dev-team/architect",
    workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
  });
}

function bytes(writes: unknown[]): number[][] {
  return writes.map((write) => Array.from(write as Uint8Array));
}

describe("TerminalView snapshot render gate (#955)", () => {
  let cleanupDom: (() => void) | null = null;
  let warn: ReturnType<typeof vi.spyOn>;
  let debug: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    debug = vi.spyOn(console, "debug").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
    warn.mockRestore();
    debug.mockRestore();
  });

  // THE REGRESSION. Red against main: the live chunk is pushed onto
  // pendingSnapshotEvents and never written, so `writes` stays empty forever —
  // the black tile. Green with the gate removed.
  it("renders live PTY output while the snapshot round-trip is still in flight", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    // Never settles: exactly the state the Trace capture proved.
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      expect(bytes(terminal.screen)).toEqual([LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // The trace showed ~40 chunks over 5.4 s into a black tile. Every one of them
  // must reach the screen while the round-trip is still outstanding.
  it("keeps rendering every live chunk while the snapshot never settles", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      for (let index = 0; index < 5; index += 1) {
        fake.emitFromBackend("pty_output", {
          sessionId: SESSION_A,
          data: [65 + index],
          sequence: index + 1,
        });
      }

      await waitFor(() => expect(terminal.writes).toHaveLength(5));
      expect(bytes(terminal.screen)).toEqual([[65], [66], [67], [68], [69]]);
    } finally {
      rendered.cleanup();
    }
  });

  // Safety net, not a gate: a terminal that has rendered nothing at all says so
  // instead of sitting black and silent. Red against main (no status is ever
  // surfaced while the promise is outstanding).
  it("surfaces the unavailable status when the snapshot stalls with nothing rendered", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));

      await waitFor(() => {
        const status = rendered.root.querySelector<HTMLDivElement>(
          '[data-ac-testid="terminal.replay-status.11111111-1111-4111-8111-111111111111"]'
        );
        expect(status?.hidden).toBe(false);
      }, 3000);

      expect(warn).toHaveBeenCalledWith(
        expect.stringContaining("still pending after 500ms")
      );
    } finally {
      rendered.cleanup();
    }
  });

  // ...but a terminal that IS rendering live output has nothing to report.
  it("does not surface the unavailable status while live output is rendering", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      await new Promise((resolve) => setTimeout(resolve, 700));

      const status = rendered.root.querySelector<HTMLDivElement>(
        '[data-ac-testid="terminal.replay-status.11111111-1111-4111-8111-111111111111"]'
      );
      expect(status?.hidden).toBe(true);
      expect(bytes(terminal.screen)).toEqual([LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // RE-ATTACH, snapshot loses the race. The snapshot is a full-screen repaint,
  // so it cannot simply be written on top of live bytes. The screen is rebuilt:
  // reset -> snapshot (everything <= its sequence) -> live events after it.
  // Here the snapshot already contains the live chunk (sequence 1), so the
  // chunk must NOT be replayed on top of it: no duplication.
  it("rebuilds from the snapshot without duplicating live output it already contains", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });

      // The gate is gone: it is on screen before the snapshot settles.
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      snapshot.resolve({
        sessionId: SESSION_A,
        data: [...SNAP, ...LIVE],
        rows: null,
        cols: null,
        sequence: 1, // the snapshot's screen already includes event #1
      });

      await waitFor(() => expect(terminal.resets).toBe(1));
      await flushPromises();

      // Visible screen: the snapshot alone. The live chunk is inside it and is
      // dropped by the sequence dedup rather than written a second time.
      expect(bytes(terminal.screen)).toEqual([[...SNAP, ...LIVE]]);
      // History proves the live byte was rendered immediately, before the
      // snapshot arrived — that is the whole point of the fix.
      expect(bytes(terminal.writes)).toEqual([LIVE, [...SNAP, ...LIVE]]);
    } finally {
      rendered.cleanup();
    }
  });

  // RE-ATTACH, snapshot loses the race and does NOT cover the newest live
  // event. The rebuild must replay everything after the snapshot's sequence:
  // no missing output.
  it("replays the live output the late snapshot does not cover", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: NEXT,
        sequence: 2,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));

      snapshot.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 1, // covers event #1 only
      });

      await waitFor(() => expect(terminal.resets).toBe(1));
      await flushPromises();

      // Snapshot, then only the event it did not contain. Event #1 is dropped
      // (already in the snapshot), event #2 is replayed (it was not).
      expect(bytes(terminal.screen)).toEqual([SNAP, NEXT]);
    } finally {
      rendered.cleanup();
    }
  });

  // RE-ATTACH, snapshot WINS the race (the normal, fast path). #1363 changed
  // one thing here: the seed is applied with a reset even on a clean terminal,
  // because every attach re-seeds and the reset is what makes replaying the
  // 64 KiB history ring safe (plan 3.4.2 / 3.3 rule 2).
  it("seeds a clean terminal from the snapshot with exactly one reset", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      snapshot.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: NEXT,
        sequence: 2,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));

      expect(terminal.resets).toBe(1);
      expect(bytes(terminal.screen)).toEqual([SNAP, NEXT]);
    } finally {
      rendered.cleanup();
    }
  });

  // The retention budget bounds memory when the round-trip never settles. Once
  // it is spent the live events after the snapshot's sequence are no longer
  // held, so a rebuild would DROP them: the snapshot must be discarded instead,
  // and the live screen kept.
  it("discards a snapshot that lands after the reconcile budget is spent", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      // One chunk larger than the 2 MiB retention budget.
      const flood = new Array<number>(2 * 1024 * 1024 + 1).fill(65);
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: flood,
        sequence: 1,
      });
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: NEXT,
        sequence: 2,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));

      snapshot.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 1,
      });

      await flushPromises();
      await flushPromises();

      // No reset: the live screen survives intact, snapshot dropped.
      expect(terminal.resets).toBe(0);
      expect(bytes(terminal.screen.slice(1))).toEqual([NEXT]);
      expect(warn).toHaveBeenCalledWith(
        expect.stringContaining("outran the reconcile budget")
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("disposes xterm when the real render gate closes and leaves queued work inert", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1),
      );
      const terminal = xterm.instances[0];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("session_switched", dormantSelection(SESSION_A, 2));
      await waitFor(() => expect(terminal.disposed).toBe(true));
      const before = {
        writes: terminal.writes.length,
        screen: terminal.screen.length,
        resets: terminal.resets,
        resizes: terminal.resizes.length,
        ptyResizes: fake.callsFor("pty_resize").length,
      };

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: NEXT,
        sequence: 2,
      });
      snapshot.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: 24,
        cols: 80,
        sequence: 1,
      });
      await flushPromises();
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

      expect(terminal.disposed).toBe(true);
      expect({
        writes: terminal.writes.length,
        screen: terminal.screen.length,
        resets: terminal.resets,
        resizes: terminal.resizes.length,
        ptyResizes: fake.callsFor("pty_resize").length,
      }).toEqual(before);
      expect(() => terminal.resize(90, 30)).toThrow("terminal disposed");
    } finally {
      rendered.cleanup();
    }
  });
});
