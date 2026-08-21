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
import {
  TEST_TERMINAL_DOCUMENT_EPOCH,
  terminalActivationWire,
  terminalSeedlessActivationWire,
  type TerminalSnapshotWireOptions,
} from "../../shared/testing/terminal-output";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { initialSelection, liveSelection, SESSION_A } from "../../shared/testing/session-selection";

vi.mock("../../sidebar/stores/sessions", () => ({
  sessionsStore: {
    resetContextReadingsForTests: () => undefined,
    setSessions: () => undefined,
    resetSelectionForTests: () => undefined,
    setTeams: () => undefined,
    setRepos: () => undefined,
    setAlwaysShowSelectedWorkgroup: () => undefined,
    setCoordSortByActivity: () => undefined,
    clearDetached: () => undefined,
  },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => undefined }),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "terminal" }),
}));

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: unknown[];
  screen: unknown[];
  resets: number;
  resizes: { cols: number; rows: number }[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
  reset(): void;
  readonly buffer: {
    readonly active: {
      readonly type: "normal";
      readonly viewportY: number;
      readonly baseY: number;
      readonly length: number;
      readonly getLine: (index: number) =>
        | { readonly getCell: (col: number) => { readonly getChars: () => string } | undefined }
        | undefined;
    };
  };
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
    resizes: { cols: number; rows: number }[] = [];
    private viewportY = 0;
    private baseY = 0;
    private resizeHandlers = new Set<(size: { cols: number; rows: number }) => void>();

    constructor() {
      xterm.instances.push(this);
    }

    loadAddon(addon?: { activate?: (terminal: FakeTerminalInstance) => void }): void {
      addon?.activate?.(this);
    }

    open(container: HTMLElement): void {
      const element = document.createElement("div");
      element.className = "xterm";
      const screen = document.createElement("div");
      screen.className = "xterm-screen";
      const canvas = document.createElement("canvas");
      canvas.width = 880;
      canvas.height = 520;
      screen.appendChild(canvas);
      element.appendChild(screen);
      container.appendChild(element);
      const rect = () => new DOMRect(0, 0, 880, 520);
      container.getBoundingClientRect = rect;
      element.getBoundingClientRect = rect;
      screen.getBoundingClientRect = rect;
      this.element = element;
    }

    focus(): void {}

    dispose(): void {
      this.resizeHandlers.clear();
    }

    write(data: unknown, callback?: () => void): void {
      if (data instanceof Uint8Array && data.length === 0) {
        callback?.();
        return;
      }
      this.writes.push(data);
      this.screen.push(data);
      callback?.();
    }

    /** Real xterm RIS: clears the screen and scrollback. */
    reset(): void {
      this.resets += 1;
      this.screen.length = 0;
    }

    scrollToBottom(): void {
      this.viewportY = this.baseY;
    }

    get buffer() {
      return {
        active: {
          type: "normal" as const,
          viewportY: this.viewportY,
          baseY: this.baseY,
          length: this.baseY + this.rows,
          getLine: (index: number) => {
            if (index < this.baseY || index >= this.baseY + this.rows) return undefined;
            return {
              getCell: (col: number) => {
                if (col < 0 || col >= this.cols) return undefined;
                const hasText = this.screen.some(
                  (value) => value instanceof Uint8Array && value.length > 0,
                );
                return { getChars: () => (hasText && index === this.baseY && col === 0 ? "x" : "") };
              },
            };
          },
        },
      };
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

    fit = vi.fn(() => {
      this.terminal?.resize(fitViewport.cols, fitViewport.rows);
    });

    proposeDimensions(): { cols: number; rows: number } {
      return { ...fitViewport };
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
  fake.resolve("terminal_output_document_epoch", TEST_TERMINAL_DOCUMENT_EPOCH);
  fake.onInvoke("activate_terminal_output", (args) => terminalSeedlessActivationWire(args));
  fake.resolve("detach_terminal_output", undefined);
  fake.resolve("cancel_terminal_output_activation", undefined);
  fake.resolve("record_terminal_attach_observation", undefined);
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

function deferSnapshotActivation(fake: FakeTransport): {
  readonly resolve: (options: TerminalSnapshotWireOptions) => void;
} {
  const pending = deferred<unknown>();
  let request: Record<string, unknown> | null = null;
  fake.onInvoke("activate_terminal_output", (args) => {
    request = args;
    return pending.promise;
  });
  return {
    resolve: (options) => {
      if (request === null) throw new Error("activation request has not started");
      pending.resolve(terminalActivationWire(request, options));
    },
  };
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
    const snapshot = deferred<unknown>();
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
      await rendered.cleanupAsync();
    }
  });

  // The trace showed ~40 chunks over 5.4 s into a black tile. Every one of them
  // must reach the screen while the round-trip is still outstanding.
  it("keeps rendering every live chunk while the snapshot never settles", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<unknown>();
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
      await rendered.cleanupAsync();
    }
  });

  // Safety net, not a gate: a terminal that has rendered nothing at all says so
  // instead of sitting black and silent. Red against main (no status is ever
  // surfaced while the promise is outstanding).
  it("surfaces the unavailable status when the attachment deadline expires with nothing rendered", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    const snapshot = deferred<unknown>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      await flushPromises();
      await flushPromises();
      expect(xterm.instances).toHaveLength(1);

      await vi.advanceTimersByTimeAsync(5_001);
      await flushPromises();

      const status = rendered.root.querySelector<HTMLDivElement>(
        '[data-ac-testid="terminal.replay-status.11111111-1111-4111-8111-111111111111"]'
      );
      expect(status?.hidden).toBe(false);

      expect(fake.lastCall("record_terminal_attach_observation")?.args.observation).toMatchObject(
        { stage: "aborted", outcome: "timeout" },
      );
    } finally {
      await rendered.cleanupAsync();
      vi.useRealTimers();
    }
  });

  // ...but a terminal that IS rendering live output has nothing to report.
  it("does not surface the unavailable status while live output is rendering", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<unknown>();
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
      await rendered.cleanupAsync();
    }
  });

  // RE-ATTACH, snapshot loses the race. The snapshot is a full-screen repaint,
  // so it cannot simply be written on top of live bytes. The screen is rebuilt:
  // reset -> snapshot (everything <= its sequence) -> live events after it.
  // Here the snapshot already contains the live chunk (sequence 1), so the
  // chunk must NOT be replayed on top of it: no duplication.
  it("rebuilds from the snapshot without duplicating live output it already contains", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const snapshot = deferSnapshotActivation(fake);

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
        replayData: [...SNAP, ...LIVE],
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
      await rendered.cleanupAsync();
    }
  });

  // RE-ATTACH, snapshot loses the race and does NOT cover the newest live
  // event. The rebuild must replay everything after the snapshot's sequence:
  // no missing output.
  it("replays the live output the late snapshot does not cover", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const snapshot = deferSnapshotActivation(fake);

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
        replayData: SNAP,
        sequence: 1, // covers event #1 only
      });

      await waitFor(() => expect(terminal.resets).toBe(1));
      await flushPromises();

      // Snapshot, then only the event it did not contain. Event #1 is dropped
      // (already in the snapshot), event #2 is replayed (it was not).
      expect(bytes(terminal.screen)).toEqual([SNAP, NEXT]);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  // RE-ATTACH, snapshot WINS the race (the normal, fast path). #1363 changed
  // one thing here: the seed is applied with a reset even on a clean terminal,
  // because every attach re-seeds and the reset is what makes replaying the
  // 64 KiB history ring safe (plan 3.4.2 / 3.3 rule 2).
  it("seeds a clean terminal from the snapshot with exactly one reset", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const snapshot = deferSnapshotActivation(fake);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      snapshot.resolve({
        replayData: SNAP,
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
      await rendered.cleanupAsync();
    }
  });

  // The retention budget bounds memory when the round-trip never settles. Once
  // it is spent the live events after the snapshot's sequence are no longer
  // held, so a rebuild would DROP them: the snapshot must be discarded instead,
  // and the live screen kept.
  it("discards a snapshot that lands after the reconcile budget is spent", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const snapshot = deferSnapshotActivation(fake);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output")).toHaveLength(1)
      );

      const terminal = xterm.instances[0];
      // Five individually valid chunks cross the 2 MiB reconcile budget.
      const flood = new Array<number>(512 * 1024).fill(65);
      for (let sequence = 1; sequence <= 5; sequence += 1) {
        fake.emitFromBackend("pty_output", {
          sessionId: SESSION_A,
          data: flood,
          sequence,
        });
      }
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: NEXT,
        sequence: 6,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(6));

      snapshot.resolve({
        replayData: SNAP,
        sequence: 5,
      });

      await flushPromises();
      await flushPromises();

      // No reset: the live screen survives intact, snapshot dropped.
      expect(terminal.resets).toBe(0);
      expect(bytes(terminal.screen.slice(-1))).toEqual([NEXT]);
    } finally {
      await rendered.cleanupAsync();
    }
  });
});
