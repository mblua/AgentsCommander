// @vitest-environment jsdom
//
// #1283 — the terminal render gate, updated to the activation protocol.
//
// The legacy #955 contract ("a live PTY byte is rendered on arrival, never
// gated behind a snapshot round-trip") is superseded by the #1283 contract:
// the backend refuses to emit current-generation data or markers until the
// frontend has synchronously committed the exact activation payload and
// accepted readiness, so the frontend barrier is the Activating window itself.
//
// These tests lock down the new gate:
//   1. No byte reaches xterm, ready, or ack while Activating (pre-promise).
//   2. The activated path renders ONLY the exact activation payload: the
//      legacy snapshot provider is never consulted, even when it could return
//      newer S+1 content, and retained S+1 writes exactly once after replay.
//   3. A terminal that has rendered nothing says so when replay stalls; one
//      that IS rendering has nothing to report.
//   4. Snapshot-represented deliveries never write twice; a post-snapshot gap
//      seals without acknowledgement and recovers exactly once.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import type {
  TerminalOutputActivationResult,
  TerminalOutputControlState,
} from "../../shared/types";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { initialSelection, liveSelection, SESSION_A } from "../../shared/testing/session-selection";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: unknown[];
  screen: unknown[];
  resets: number;
  resizes: { cols: number; rows: number }[];
  holdNextWrites: number;
  heldWrites: { bytes: number[]; callback: (() => void) | null }[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
  reset(): void;
  releaseHeld(index?: number): void;
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
    holdNextWrites = 0;
    heldWrites: { bytes: number[]; callback: (() => void) | null }[] = [];
    private resizeHandlers = new Set<(size: { cols: number; rows: number }) => void>();

    constructor() {
      xterm.instances.push(this);
    }

    loadAddon(): void {}

    open(element: HTMLElement): void {
      this.element = element;
    }

    focus(): void {}

    dispose(): void {
      this.resizeHandlers.clear();
    }

    write(data: unknown, callback?: () => void): void {
      const record = { bytes: Array.from(data as Uint8Array), callback: callback ?? null };
      this.writes.push(data);
      this.screen.push(data);
      if (this.holdNextWrites > 0) {
        this.holdNextWrites -= 1;
        this.heldWrites.push(record);
        return;
      }
      callback?.();
    }

    releaseHeld(index = 0): void {
      this.heldWrites[index]?.callback?.();
    }

    /** Real xterm RIS: clears the screen and scrollback. */
    reset(): void {
      this.resets += 1;
      this.screen.length = 0;
    }

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

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;

    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }

    fit = vi.fn(() => {
      this.terminal?.resize(fitViewport.cols, fitViewport.rows);
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

const SNAP = [83, 78, 65, 80]; // "SNAP"
const LIVE = [76, 73, 86, 69]; // "LIVE"
const NEXT = [78, 69, 88, 84]; // "NEXT"

const GENERATION = "1";

function setupTerminalTransport(fake: FakeTransport, sessions = [session()]): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve(
    "get_active_session",
    sessions[0] ? liveSelection(sessions[0].id) : initialSelection(),
  );
  fake.onInvoke("list_sessions", () => sessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  // #1283: activation payload is the only snapshot source. The legacy provider
  // is wired to return a NEWER snapshot containing S+1: the activated path
  // must never consult it.
  fake.onInvoke("get_screen_snapshot", ({ sessionId }) => ({
    sessionId,
    data: [...SNAP, ...LIVE],
    rows: null,
    cols: null,
    sequence: 1,
  }));
  const generationBySession = new Map<string, string>();
  fake.onInvoke("activate_terminal_output", (args) => {
    const sessionId = String(args.sessionId);
    const next = (parseInt(generationBySession.get(sessionId) ?? "0", 10) || 0) + 1;
    generationBySession.set(sessionId, String(next));
    const instance = xterm.instances.find(
      (candidate) =>
        candidate.element?.getAttribute("data-ac-session-id") === sessionId,
    );
    return {
      kind: "activated",
      activation: {
        sessionId,
        generation: String(next),
        snapshot: {
          data: SNAP,
          rows: instance?.rows ?? 24,
          cols: instance?.cols ?? 80,
          sequence: "0",
        },
      },
    } as TerminalOutputActivationResult;
  });
  fake.onInvoke("ready_terminal_output", (args) => ({
    kind: "active",
    sessionId: String(args.sessionId),
    generation: String(args.generation),
  }));
  fake.onInvoke("deactivate_terminal_output", (args) => ({
    kind: "inactive",
    sessionId: String(args.sessionId),
    generation: String(args.generation),
  }));
  fake.resolve("ack_terminal_output_delivery", { kind: "stale" });
  fake.onInvoke("report_terminal_renderer_metrics", (args) => ({
    kind: "active",
    sessionId: String(args.sessionId),
    generation: String(args.generation),
  }));
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

function statusFor(root: HTMLElement): HTMLDivElement | null {
  return root.querySelector<HTMLDivElement>(
    '[data-ac-testid="terminal.replay-status.11111111-1111-4111-8111-111111111111"]',
  );
}

describe("TerminalView activation render gate (#1283)", () => {
  let cleanupDom: (() => void) | null = null;
  let warn: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
    warn.mockRestore();
  });

  // THE GATE. While the activation promise is outstanding, current-generation
  // data and a resync marker are parsed but rejected: no xterm write, no ready
  // call, no acknowledgement. After the exact local commit, readiness carries
  // S, and only then does the payload reach xterm.
  it("renders nothing before the activation commits and readiness accepts", async () => {
    const fake = new FakeTransport();
    const activation = deferred<TerminalOutputActivationResult>();
    setupTerminalTransport(fake, [onlySession()]);
    fake.onInvoke("activate_terminal_output", () => activation.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];

      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: LIVE,
      });
      fake.emitFromBackend("pty_output", {
        kind: "resyncRequired",
        sessionId: SESSION_A,
        generation: GENERATION,
        sequence: "0",
      });

      expect(terminal.writes).toHaveLength(0);
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(0);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);

      activation.resolve({
        kind: "activated",
        activation: {
          sessionId: SESSION_A,
          generation: GENERATION,
          snapshot: { data: SNAP, rows: 24, cols: 80, sequence: "0" },
        },
      });
      await flushPromises();

      // The ReplayPending commit precedes readiness: one ready call with S.
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(1);
      expect(fake.lastCall("ready_terminal_output")?.args).toEqual({
        sessionId: SESSION_A,
        generation: GENERATION,
        snapshotSequence: "0",
      });

      // Ready accepted: the exact activation payload renders.
      await flushPromises();
      expect(bytes(terminal.writes)).toEqual([SNAP]);

      // The bytes rejected during Activating are NOT replayed; a fresh S+1
      // delivery is retained and drains exactly once after replay.
      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: LIVE,
      });
      await waitFor(() => expect(bytes(terminal.writes)).toEqual([SNAP, LIVE]));
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  // The legacy provider could return a newer snapshot containing S+1. The
  // activated path must make ZERO legacy fetches, use S as the filter anchor,
  // and write retained S+1 exactly once after the replay callback.
  it("renders only the exact activation payload and never consults the legacy snapshot", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const activation = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", () => activation.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      terminal.holdNextWrites = 1; // hold the snapshot write: ReplayPending
      activation.resolve({
        kind: "activated",
        activation: {
          sessionId: SESSION_A,
          generation: GENERATION,
          snapshot: { data: SNAP, rows: 24, cols: 80, sequence: "0" },
        },
      });

      await flushPromises();
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);
      expect(bytes(terminal.writes)).toEqual([SNAP]);
      expect(terminal.heldWrites[0].bytes).toEqual(SNAP);

      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: LIVE,
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      // Retained but never written while ReplayPending.
      expect(bytes(terminal.writes)).toEqual([SNAP]);

      terminal.releaseHeld(0);
      await waitFor(() => expect(bytes(terminal.writes)).toEqual([SNAP, LIVE]));
      expect(bytes(terminal.writes.filter((_, index) => index > 0))).toEqual([LIVE]);
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);

      const acks = fake.callsFor("ack_terminal_output_delivery");
      expect(acks).toHaveLength(1);
      expect(acks[0].args).toEqual({
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
      });
    } finally {
      rendered.cleanup();
    }
  });

  // A terminal that has rendered nothing at all says so instead of sitting
  // black and silent while readiness never settles.
  it("surfaces the unavailable status when readiness stalls with nothing rendered", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);
    const ready = deferred<TerminalOutputControlState>();
    fake.onInvoke("ready_terminal_output", () => ready.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];

      await waitFor(() => {
        expect(statusFor(rendered.root)?.hidden).toBe(false);
      }, 3000);
      // Nothing rendered: the payload is never written before readiness.
      expect(terminal.writes).toHaveLength(0);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  // ...but a terminal that IS rendering has nothing to report.
  it("does not surface the unavailable status while output is rendering", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: LIVE,
      });
      await waitFor(() => expect(terminal.writes).toHaveLength(2));

      await new Promise((resolve) => setTimeout(resolve, 700));
      expect(statusFor(rendered.root)?.hidden).toBe(true);
      expect(bytes(terminal.screen)).toEqual([SNAP, LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // A delivery wholly represented by the activation snapshot is acknowledged
  // WITHOUT allocation: no duplicate write, ever.
  it("acknowledges a snapshot-represented delivery without writing it twice", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "0",
        sequence: "0",
        data: [...SNAP],
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(bytes(terminal.writes)).toEqual([SNAP]);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(1);
      expect(fake.lastCall("ack_terminal_output_delivery")?.args).toEqual({
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "0",
        sequence: "0",
      });
    } finally {
      rendered.cleanup();
    }
  });

  // A post-snapshot gap receives no normal acknowledgement and enters exactly
  // one recovery lane (deactivate + replacement activation); old-generation
  // deliveries after it are rejected without allocation.
  it("seals without acknowledgement on a post-snapshot gap and recovers once", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [onlySession()]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "3",
        sequence: "3",
        data: NEXT,
      });
      await flushPromises();
      await flushPromises();

      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(1);
      expect(fake.callsFor("deactivate_terminal_output")[0].args).toEqual({
        sessionId: SESSION_A,
        generation: GENERATION,
      });
      // The replacement activation is a fresh generation.
      await flushPromises();
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(2);

      // Old-generation events are rejected.
      fake.emitFromBackend("pty_output", {
        kind: "data",
        sessionId: SESSION_A,
        generation: GENERATION,
        firstSequence: "1",
        sequence: "1",
        data: LIVE,
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });
});
