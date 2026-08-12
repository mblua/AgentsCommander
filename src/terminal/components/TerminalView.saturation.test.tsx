// @vitest-environment jsdom
//
// #1283 - TerminalView saturation/admission suite (plan 9.3, 9.4, 14.2-14.5).
//
// Drives the real TerminalApp with mocked typed IPC (FakeTransport), Tauri
// events, and Xterm. Deterministic: fake timers drive the health deadline,
// frame scheduling (rAF shimmed to setTimeout(0)), and the sustained load.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import type {
  TerminalOutputActivationResult,
  TerminalOutputControlState,
  TerminalRendererMetrics,
} from "../../shared/types";
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

interface RecordedWrite {
  bytes: number[];
  callback: (() => void) | null;
}

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: RecordedWrite[];
  screen: number[][];
  resets: number;
  disposed: boolean;
  holdNextWrites: number;
  heldWrites: RecordedWrite[];
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
  releaseHeld(index?: number): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

const fitViewport = vi.hoisted(() => ({ cols: 88, rows: 26 }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    writes: RecordedWrite[] = [];
    screen: number[][] = [];
    resets = 0;
    disposed = false;
    holdNextWrites = 0;
    heldWrites: RecordedWrite[] = [];
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
      this.disposed = true;
      this.resizeHandlers.clear();
    }

    write(data: unknown, callback?: () => void): void {
      const bytes = Array.from(data as Uint8Array);
      const record: RecordedWrite = { bytes, callback: callback ?? null };
      this.writes.push(record);
      this.screen.push(bytes);
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

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function activated(
  sessionId: string,
  generation: string,
  snapshotSequence: string,
  data: number[] = [83, 78, 65, 80]
): TerminalOutputActivationResult {
  return {
    kind: "activated",
    activation: {
      sessionId,
      generation,
      snapshot: {
        data,
        rows: 24,
        cols: 80,
        sequence: snapshotSequence,
      },
    },
  };
}

const active = (sessionId: string, generation: string): TerminalOutputControlState => ({
  kind: "active",
  sessionId,
  generation,
});

const inactive = (sessionId: string, generation: string): TerminalOutputControlState => ({
  kind: "inactive",
  sessionId,
  generation,
});

interface ProtocolHandles {
  /** Current backend generation per session (read after an activation settles). */
  generations: Map<string, string>;
}

/** Standard protocol backend: activate -> activated(generation N, S=N*10-5),
 *  ready -> active, deactivate -> inactive, ack -> stale, report -> active.
 *  The legacy snapshot provider returns a NEWER snapshot containing S+1; the
 *  activated path must never call it. */
function installProtocol(fake: FakeTransport, sessions: string[]): ProtocolHandles {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", liveSelection(sessions[0] ?? SESSION_A));
  fake.onInvoke("list_sessions", () => sessions.map((id) => session({ id })));
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.onInvoke("get_screen_snapshot", (args) => ({
    sessionId: String(args.sessionId),
    data: [89, 89, 89, 89],
    rows: null,
    cols: null,
    sequence: 999,
  }));

  const generations = new Map<string, string>();
  fake.onInvoke("activate_terminal_output", (args) => {
    const sessionId = String(args.sessionId);
    const next = (parseInt(generations.get(sessionId) ?? "0", 10) || 0) + 1;
    const generation = String(next);
    generations.set(sessionId, generation);
    return activated(sessionId, generation, String(next * 10 - 5));
  });
  fake.onInvoke("ready_terminal_output", (args) =>
    active(String(args.sessionId), String(args.generation)),
  );
  fake.onInvoke("deactivate_terminal_output", (args) =>
    inactive(String(args.sessionId), String(args.generation)),
  );
  fake.resolve("ack_terminal_output_delivery", { kind: "stale" });
  fake.onInvoke("report_terminal_renderer_metrics", (args) =>
    active(String(args.sessionId), String(args.generation)),
  );
  return { generations };
}

let currentFake: FakeTransport | null = null;
function emitPtyOutput(
  payload:
    | { kind: "data"; sessionId: string; generation: string; firstSequence: string; sequence: string; data: number[] }
    | { kind: "resyncRequired"; sessionId: string; generation: string; sequence: string }
): void {
  currentFake?.emitFromBackend("pty_output", payload);
}

function metricsReports(fake: FakeTransport): TerminalRendererMetrics[] {
  return fake
    .callsFor("report_terminal_renderer_metrics")
    .map((call) => call.args.metrics as TerminalRendererMetrics);
}

function instanceFor(sessionId: string): FakeTerminalInstance {
  const instance = xterm.instances.find(
    (candidate) =>
      !candidate.disposed &&
      candidate.element?.getAttribute("data-ac-session-id") === sessionId,
  );
  if (!instance) {
    throw new Error(`no live xterm instance for ${sessionId}; have ${xterm.instances.length}`);
  }
  return instance;
}

/** Real-timer settling: wait for the initial terminal instance. */
async function settleReal(): Promise<void> {
  await waitFor(() => expect(xterm.instances.length).toBeGreaterThan(0));
  await flushMicrotasks();
}

/** Fake-timer settling: flush microtasks and zero/one-ms timers until the
 *  initial terminal instance exists. Never uses wall-clock waiting. */
async function settleFake(): Promise<void> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    await flushMicrotasks();
    await vi.advanceTimersByTimeAsync(1);
    if (xterm.instances.length > 0) {
      await flushMicrotasks();
      return;
    }
  }
  throw new Error("no terminal instance after settling fake timers");
}

/**
 * Fake-timer tests need a deterministic rAF backed by the FAKED setTimeout:
 * jsdom ships a native requestAnimationFrame, so the browser-stub shim is
 * never installed and its frames would run on real time instead of the fake
 * clock. Call AFTER vi.useFakeTimers().
 */
function installFakeTimersRaf(): void {
  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value: (callback: FrameRequestCallback) =>
      globalThis.setTimeout(() => callback(performance.now()), 0) as unknown as number,
  });
  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    writable: true,
    value: (handle: number) => globalThis.clearTimeout(handle),
  });
}

/** Fake-timer wait: advance until `condition` holds (bounded). */
async function waitForFake(condition: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (condition()) return;
    await flushMicrotasks();
    await vi.advanceTimersByTimeAsync(1);
  }
  throw new Error("condition not reached within fake time");
}

describe("TerminalView saturation/admission (#1283)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
  });

  afterEach(() => {
    currentFake = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
    vi.useRealTimers();
  });

  it("barriers output behind the activation commit and readiness (pre-promise readiness gate)", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    const activationGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", () => activationGate.promise);

    currentFake = fake;
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);

      // While Activating: current-generation data and a marker are parsed but
      // must NOT reach xterm, ready, or ack (the backend barrier guarantees
      // they are not emitted; the frontend must reject them anyway).
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      emitPtyOutput({
        kind: "resyncRequired",
        sessionId: SESSION_A,
        generation: "1",
        sequence: "0",
      });
      expect(terminal.writes).toHaveLength(0);
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(0);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);

      // Resolve activation; the ReplayPending commit happens BEFORE ready.
      activationGate.resolve(activated(SESSION_A, "1", "5"));
      await flushMicrotasks();
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(1);
      expect(fake.lastCall("ready_terminal_output")?.args).toEqual({
        sessionId: SESSION_A,
        generation: "1",
        snapshotSequence: "5",
      });

      // Ready resolves active: the exact activation payload is rendered, then
      // retained post-snapshot data drains once.
      await flushMicrotasks();
      expect(terminal.writes.map((write) => write.bytes)).toEqual([[83, 78, 65, 80]]);

      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      await waitFor(() =>
        expect(terminal.writes.map((write) => write.bytes)).toEqual([
          [83, 78, 65, 80],
          [66],
        ]),
      );
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("renders only the exact activation snapshot; S+1 writes exactly once, zero legacy fetches", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    const activationGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", () => activationGate.promise);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);
      terminal.holdNextWrites = 1; // hold the snapshot write: ReplayPending

      // The legacy provider would claim S+1 is already rendered: it must never
      // be consulted, and S+1 must still be written exactly once after replay.
      activationGate.resolve(activated(SESSION_A, "1", "5"));
      await flushMicrotasks();
      await flushMicrotasks();
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);
      expect(terminal.heldWrites).toHaveLength(1);
      expect(terminal.heldWrites[0].bytes).toEqual([83, 78, 65, 80]);

      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      // S+1 is retained but NEVER written while ReplayPending.
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(terminal.writes).toHaveLength(1);

      terminal.releaseHeld(0); // snapshot callback completes
      await waitFor(() =>
        expect(terminal.writes.map((write) => write.bytes)).toEqual([
          [83, 78, 65, 80],
          [66],
        ]),
      );
      // Exactly one acknowledgement for the retained range.
      const acks = fake.callsFor("ack_terminal_output_delivery");
      expect(acks).toHaveLength(1);
      expect(acks[0].args).toEqual({
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("owns exactly one write gate during a held live write and releases it on settlement", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);
      // Live: queue one batch and hold the frame write.
      terminal.holdNextWrites = 1;
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      await waitFor(() => expect(terminal.heldWrites).toHaveLength(1));

      // While held, a second delivery queues but no second write starts.
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "7",
        sequence: "7",
        data: [67],
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(terminal.writes).toHaveLength(2); // snapshot + held live write

      terminal.releaseHeld(0);
      await waitFor(() => expect(terminal.writes).toHaveLength(3));
      expect(terminal.writes[2].bytes).toEqual([67]);
    } finally {
      rendered.cleanup();
    }
  });

  it("reaches one recovery through the health deadline when the replay callback is permanently held", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] });
    installFakeTimersRaf();
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    const activationGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", () => activationGate.promise);
    fake.onInvoke("report_terminal_renderer_metrics", () => ({
      kind: "resyncRequired",
      sessionId: SESSION_A,
      generation: "1",
      sequence: "0",
    }));
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleFake();
      const terminal = instanceFor(SESSION_A);
      terminal.holdNextWrites = 1; // the replay callback will never settle
      activationGate.resolve(activated(SESSION_A, "1", "5"));
      await flushMicrotasks();
      await flushMicrotasks();
      expect(terminal.heldWrites).toHaveLength(1);
      expect(terminal.writes).toHaveLength(1);

      // No further PTY output: the marker could not be emitted either. At the
      // generation health deadline, one metrics poll observes resyncRequired.
      await vi.advanceTimersByTimeAsync(5_000);
      await flushMicrotasks();
      expect(fake.callsFor("report_terminal_renderer_metrics")).toHaveLength(1);

      // ...seals once and enters exactly one recovery lane.
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(1);
      expect(fake.lastCall("deactivate_terminal_output")?.args).toEqual({
        sessionId: SESSION_A,
        generation: "1",
      });

      // The replacement activation is a fresh generation; the held replay
      // callback is inert: no write, no ack, no extra poll.
      await flushMicrotasks();
      await flushMicrotasks();
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(2);
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(2);
      const writesBefore = terminal.writes.length;
      terminal.releaseHeld(0);
      await flushMicrotasks();
      expect(terminal.writes).toHaveLength(writesBefore);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
      expect(fake.callsFor("report_terminal_renderer_metrics")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("preserves B when gB > gA in the A-first/outstanding-B ordering", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A, SESSION_B]);
    const aGate = deferred<TerminalOutputActivationResult>();
    const bGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", (args) =>
      String(args.sessionId) === SESSION_A ? aGate.promise : bGate.promise,
    );
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      expect(instanceFor(SESSION_A).writes).toHaveLength(0);

      // Switch to B while A is still outstanding.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await flushMicrotasks();
      expect(instanceFor(SESSION_B).writes).toHaveLength(0);

      // A response arrives FIRST: creates the ReconcilePending record and
      // serializes deactivate(A, gA=1).
      aGate.resolve(activated(SESSION_A, "1", "5"));
      await flushMicrotasks();
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(1);

      // B settles with gB=2 > gA=1: exactly one B commit, no b2.
      bGate.resolve(activated(SESSION_B, "2", "15"));
      await flushMicrotasks();
      await flushMicrotasks();
      const readyCalls = fake.callsFor("ready_terminal_output");
      expect(readyCalls).toHaveLength(1);
      expect(readyCalls[0].args).toEqual({
        sessionId: SESSION_B,
        generation: "2",
        snapshotSequence: "15",
      });
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(2);
      const bTerminal = instanceFor(SESSION_B);
      await waitFor(() => expect(bTerminal.writes).toHaveLength(1));
      expect(bTerminal.writes[0].bytes).toEqual([83, 78, 65, 80]);
    } finally {
      rendered.cleanup();
    }
  });

  it("preserves an already-active B (gB > gA) when a late A success arrives", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A, SESSION_B]);
    const aGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", (args) =>
      String(args.sessionId) === SESSION_A ? aGate.promise : activated(SESSION_B, "2", "15"),
    );
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instanceFor(SESSION_B).writes).toHaveLength(1));

      // Late A success (gA=1) while B is Active(gB=2): B untouched, A
      // deactivation is stale, and NO B reissue occurs.
      const activateCalls = fake.callsFor("activate_terminal_output").length;
      aGate.resolve(activated(SESSION_A, "1", "5"));
      await flushMicrotasks();
      await flushMicrotasks();
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(1);
      expect(fake.callsFor("deactivate_terminal_output")[0].args).toEqual({
        sessionId: SESSION_A,
        generation: "1",
      });
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(activateCalls);
      expect(fake.callsFor("ready_terminal_output")).toHaveLength(1);
      expect(instanceFor(SESSION_B).writes).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("seals disposed B and issues exactly one b2 when a late A displaced it (gA > gB)", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A, SESSION_B]);
    const aGate = deferred<TerminalOutputActivationResult>();
    fake.onInvoke("activate_terminal_output", (args) =>
      String(args.sessionId) === SESSION_A ? aGate.promise : activated(SESSION_B, "1", "5"),
    );
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instanceFor(SESSION_B).writes).toHaveLength(1));
      const firstB = instanceFor(SESSION_B);

      // Late A success with gA=2 > gB=1: A displaced B. B/gB is synchronously
      // sealed/disposed before A teardown, then exactly one b2 runs.
      aGate.resolve(activated(SESSION_A, "2", "15"));
      await flushMicrotasks();
      await flushMicrotasks();
      expect(firstB.disposed).toBe(true);
      expect(fake.callsFor("deactivate_terminal_output").length).toBeGreaterThanOrEqual(2);

      await flushMicrotasks();
      const activateCalls = fake.callsFor("activate_terminal_output");
      expect(activateCalls).toHaveLength(3); // A, B, then exactly one b2
      expect(activateCalls[2].args).toEqual({ sessionId: SESSION_B });

      await flushMicrotasks();
      const b2 = instanceFor(SESSION_B);
      expect(b2).not.toBe(firstB);
      await waitFor(() => expect(b2.writes).toHaveLength(1));
    } finally {
      rendered.cleanup();
    }
  });

  it("seals and recovers exactly once on a sequence gap; stale old-generation events are rejected", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);
      await waitFor(() => expect(terminal.writes).toHaveLength(1)); // snapshot live

      // Gap: firstSequence 8 != next 6.
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "8",
        sequence: "8",
        data: [72],
      });
      await flushMicrotasks();
      await flushMicrotasks();

      // Exactly one recovery lane: deactivate(generation 1), then replacement.
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(1);
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(2);
      await waitFor(() =>
        expect(instanceFor(SESSION_A).writes).toHaveLength(2),
      );

      // Old-generation events are rejected without allocation or ack.
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
      expect(instanceFor(SESSION_A).writes).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("leaves zero retired state after destroy with a permanently held write callback", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A]);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);
      terminal.holdNextWrites = 1;
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      await waitFor(() => expect(terminal.heldWrites).toHaveLength(1));

      // Destroy while the write callback is permanently held.
      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      await flushMicrotasks();
      expect(terminal.disposed).toBe(true);
      const acksBefore = fake.callsFor("ack_terminal_output_delivery").length;
      const reportsBefore = fake.callsFor("report_terminal_renderer_metrics").length;

      // Forcing the late callback is a harmless no-op: no write/ack/metrics.
      terminal.releaseHeld(0);
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(acksBefore);
      expect(fake.callsFor("report_terminal_renderer_metrics")).toHaveLength(reportsBefore);
      expect(terminal.writes).toHaveLength(2); // snapshot + held live write
    } finally {
      rendered.cleanup();
    }
  });

  it("records detach without creating a hidden terminal and rejects its events", async () => {
    const fake = new FakeTransport();
    installProtocol(fake, [SESSION_A, SESSION_B]);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      const terminal = instanceFor(SESSION_A);
      const instancesBefore = xterm.instances.length;

      // Plan 5.5.3: onTerminalDetached records detached state only; it must
      // not create a hidden Xterm/WebGL terminal merely because the event
      // arrived.
      fake.emitFromBackend("terminal_detached", {
        sessionId: SESSION_B,
        windowLabel: "terminal-B",
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(xterm.instances).toHaveLength(instancesBefore);

      // A detached active session rejects every delivery: no write, no ack.
      fake.emitFromBackend("terminal_detached", {
        sessionId: SESSION_A,
        windowLabel: "terminal-A",
      });
      emitPtyOutput({
        kind: "data",
        sessionId: SESSION_A,
        generation: "1",
        firstSequence: "6",
        sequence: "6",
        data: [66],
      });
      await new Promise((resolve) => setTimeout(resolve, 20));
      expect(terminal.writes).toHaveLength(1); // snapshot only
      expect(fake.callsFor("ack_terminal_output_delivery")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("sustained 16-session load: bounded registry, only the visible terminal writes", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] });
    installFakeTimersRaf();
    const sessions = Array.from({ length: 16 }, (_, index) =>
      `55555555-0000-4000-8000-${String(index + 1).padStart(12, "0")}`,
    );
    const fake = new FakeTransport();
    const protocol = installProtocol(fake, sessions);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleFake();
      const chunk = new Array<number>(8 * 1024).fill(65);

      // 10 seconds of fake time, switching every 500 ms. Each session's
      // admission anchors at its own S = generation*10-5, so its wave must
      // start at S+1 = generation*10-4.
      for (let switchIndex = 0; switchIndex < 20; switchIndex += 1) {
        const sessionId = sessions[switchIndex % sessions.length];
        terminalStore.setActiveSessionForTests(sessionId);
        // Settle the activation deterministically: the snapshot write landing
        // on the visible instance means the generation is Live.
        await waitForFake(() => {
          const visible = xterm.instances.find(
            (instance) =>
              !instance.disposed &&
              instance.element?.getAttribute("data-ac-session-id") === sessionId,
          );
          return visible !== undefined && visible.writes.length >= 1;
        });

        const visible = instanceFor(sessionId);
        const writesBefore = new Map(
          xterm.instances.map((instance) => [instance, instance.writes.length]),
        );
        const generation = protocol.generations.get(sessionId)!;
        const base = parseInt(generation, 10) * 10 - 4; // S + 1

        // 16 chunks of 8 KiB per switch wave (bounded below the 131,072 cap).
        for (let wave = 0; wave < 16; wave += 1) {
          const sequence = base + wave;
          emitPtyOutput({
            kind: "data",
            sessionId,
            generation,
            firstSequence: String(sequence),
            sequence: String(sequence),
            data: chunk,
          });
        }
        await vi.advanceTimersByTimeAsync(1);

        // Only the visible terminal received writes for this wave.
        for (const [instance, count] of writesBefore) {
          if (instance === visible) {
            expect(instance.writes.length).toBeGreaterThan(count);
          } else {
            expect(instance.writes.length).toBe(count);
          }
        }
        await vi.advanceTimersByTimeAsync(500);
      }

      // Registry stayed at or below four retained terminals; gauges bounded.
      const reports = metricsReports(fake);
      expect(reports.length).toBeGreaterThan(0);
      for (const report of reports) {
        expect(report.retainedTerminalCount).toBeLessThanOrEqual(4);
        expect(report.visibleTerminalCount).toBeLessThanOrEqual(1);
        expect(report.livePendingBytes + report.writeInFlightBytes).toBeLessThanOrEqual(
          131_072,
        );
        expect(report.combinedAdmissionHighWaterBytes).toBeLessThanOrEqual(131_072);
      }

      // A forced cap breach emits exactly one recovery for its generation.
      const breachSession = sessions[0];
      const breachGenBefore = protocol.generations.get(breachSession);
      const breachInstance = instanceFor(breachSession);
      const breachWritesBefore = breachInstance.writes.length;
      terminalStore.setActiveSessionForTests(breachSession);
      await waitForFake(() => {
        // The re-selection must have committed: its generation advanced AND a
        // new snapshot write landed (old writes alone are not enough).
        const generation = protocol.generations.get(breachSession);
        return (
          generation !== undefined &&
          generation !== breachGenBefore &&
          instanceFor(breachSession).writes.length > breachWritesBefore
        );
      });
      const breachGeneration = protocol.generations.get(breachSession)!;
      const breachBase = parseInt(breachGeneration, 10) * 10 - 4;
      const deactivationsBefore = fake.callsFor("deactivate_terminal_output").length;
      emitPtyOutput({
        kind: "data",
        sessionId: breachSession,
        generation: breachGeneration,
        firstSequence: String(breachBase),
        sequence: String(breachBase),
        data: new Array<number>(131_073).fill(66),
      });
      await flushMicrotasks();
      await flushMicrotasks();
      expect(fake.callsFor("deactivate_terminal_output")).toHaveLength(
        deactivationsBefore + 1,
      );
      await flushMicrotasks();

      // Unmount: nothing scheduled remains; every instance disposed.
      console.log("TIMER-DEBUG before cleanup:", vi.getTimerCount());
      rendered.cleanup();
      console.log("TIMER-DEBUG after cleanup:", vi.getTimerCount());
      await vi.advanceTimersByTimeAsync(20_000);
      console.log("TIMER-DEBUG after 20s:", vi.getTimerCount());
      expect(xterm.instances.every((instance) => instance.disposed)).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });
});
