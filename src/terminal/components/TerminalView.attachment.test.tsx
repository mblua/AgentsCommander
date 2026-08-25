// @vitest-environment jsdom
//
// #1363 - the frontend attachment surface. Three properties the restored #961
// seed/reconcile suite does not cover, because F introduces them:
//
//   1. ORDERING. Attach and detach are async invokes whose completion order is
//      not their call order. A `detach(A)` from an earlier transition landing
//      after a later `attach(A)` would leave the window displaying a session
//      it is no longer attached to: a silent freeze indistinguishable from
//      #1363 itself. The per-window promise chain plus the desired-state check
//      after every await is what prevents it, and it is load-bearing, not
//      belt-and-braces (plan 5.1).
//   2. A REJECTED INVOKE MUST NOT POISON the chain, or one transport failure
//      freezes every later transition for the life of the window.
//   3. RE-ATTACH CONTENT. Every attach re-seeds with a reset, so the output a
//      session produced while this window was detached appears — contiguously,
//      with no gap and no duplicated block (plan 3.4.2).
//
// Plus the per-window write filter with two mounted views, which is as much of
// the two-window criterion as one jsdom process can honestly express: see the
// comment on that test.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalView from "./TerminalView";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  installDeterministicAnimationFrames,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import {
  executeUiTerminalController,
  rememberSpawnViewport,
  resetUiTerminalControllerForTests,
} from "../../shared/terminal-viewport";
import { terminalStore } from "../stores/terminal";
import { SESSION_A, SESSION_B } from "../../shared/testing/session-selection";
import {
  MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT,
  type MainTerminalLayoutPulseReason,
  type MainTerminalLayoutPulseRequest,
  type MainTerminalLayoutPulseResult,
  type MainTerminalLayoutPulseStatus,
  type PtyScreenSnapshot,
} from "../../shared/types";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: number[][];
  screen: number[][];
  resizes: { cols: number; rows: number }[];
  resizeAttempts: { cols: number; rows: number }[];
  resizeListenerEvents: { cols: number; rows: number }[];
  ordinaryFitCalls: number;
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
  selection: boolean;
  missingLines: Set<number>;
  wrappedLines: Set<number>;
  scrollOperations: string[];
  scrollToBottomCalls: number;
  pendingWriteCallbacks: Array<() => void>;
  writeThrows: boolean;
  scrollToTop(): void;
  scrollToBottom(): void;
  scrollToLine(line: number): void;
  scrollLines(amount: number): void;
  scrollPages(amount: number): void;
  onResize(listener: (size: { cols: number; rows: number }) => void): {
    dispose: () => void;
  };
  resize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
  // Queue semantics matching xterm 6.0.0: a write is recorded in `writes` at
  // queue time, applied to `screen` and completed only when its callback runs.
  // When true, completions are scheduled on a microtask; when false, they are
  // parked in the instance's FIFO until the test drains it (gated control).
  autoCompleteWrites: true,
}));

const fitViewport = vi.hoisted(() => ({ cols: 80, rows: 24 }));

class ControllableResizeObserver implements ResizeObserver {
  readonly observed = new Set<Element>();
  disconnected = false;

  constructor(private readonly callback: ResizeObserverCallback) {
    resizeObserverControl.instances.push(this);
  }

  observe(target: Element): void {
    this.observed.add(target);
  }

  unobserve(target: Element): void {
    this.observed.delete(target);
  }

  disconnect(): void {
    this.disconnected = true;
    this.observed.clear();
  }

  deliver(): void {
    if (!this.disconnected && this.observed.size > 0) {
      this.callback([], this);
    }
  }
}

const resizeObserverControl = {
  instances: [] as ControllableResizeObserver[],
  latest(): ControllableResizeObserver {
    const observer = this.instances[this.instances.length - 1];
    if (!observer) {
      throw new Error("No ResizeObserver was constructed");
    }
    return observer;
  },
};

function installControllableResizeObserver(): () => void {
  const previous = globalThis.ResizeObserver;
  resizeObserverControl.instances.length = 0;
  Object.defineProperty(globalThis, "ResizeObserver", {
    configurable: true,
    writable: true,
    value: ControllableResizeObserver,
  });
  return () => {
    resizeObserverControl.instances.length = 0;
    Object.defineProperty(globalThis, "ResizeObserver", {
      configurable: true,
      writable: true,
      value: previous,
    });
  };
}
vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    writes: number[][] = [];
    screen: number[][] = [];
    resizes: { cols: number; rows: number }[] = [];
    resizeAttempts: { cols: number; rows: number }[] = [];
    resizeListenerEvents: { cols: number; rows: number }[] = [];
    ordinaryFitCalls = 0;
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
        getLine: (index: number) => {
          if (
            index < 0 ||
            index >= this.buffer.active.length ||
            this.missingLines.has(index)
          ) {
            return undefined;
          }
          return { isWrapped: this.wrappedLines.has(index) };
        },
      },
    };
    selection = false;
    missingLines = new Set<number>();
    wrappedLines = new Set<number>();
    scrollOperations: string[] = [];
    scrollToBottomCalls = 0;
    pendingWriteCallbacks: Array<() => void> = [];
    writeThrows = false;
    private resizeListener: ((size: { cols: number; rows: number }) => void) | null = null;

    constructor() {
      xterm.instances.push(this);
    }

    loadAddon(addon?: { activate?: (terminal: FakeTerminalInstance) => void }): void {
      addon?.activate?.(this);
    }
    open(element: HTMLElement): void {
      this.assertLive();
      this.element = element;
    }
    focus(): void {
      this.assertLive();
    }
    dispose(): void {
      this.disposed = true;
      this.resizeListener = null;
    }
    write(data: unknown, callback?: () => void): void {
      this.assertLive();
      // xterm's 50-MiB flow-control guard throws synchronously BEFORE the
      // chunk is queued; the byte was never queued, so the drain must release.
      if (this.writeThrows) {
        throw new Error("write data discarded, use flow control");
      }
      const bytes = Array.from(data as Uint8Array);
      this.writes.push(bytes);
      const complete = () => {
        this.screen.push(bytes);
        callback?.();
      };
      if (xterm.autoCompleteWrites) {
        queueMicrotask(complete);
      } else {
        this.pendingWriteCallbacks.push(complete);
      }
    }
    reset(): void {
      this.assertLive();
      this.resets += 1;
      this.screen.length = 0;
      // xterm 6.0.0: reset() does NOT discard the pending write queue, so the
      // parked completions survive; the buffer metrics are zeroed and `type`
      // is left untouched (tests set it explicitly).
      this.buffer.active.viewportY = 0;
      this.buffer.active.baseY = 0;
      this.buffer.active.length = 0;
    }
    scrollToTop(): void {
      this.assertLive();
      this.scrollOperations.push("top");
      this.buffer.active.viewportY = 0;
    }
    scrollToBottom(): void {
      this.assertLive();
      this.scrollToBottomCalls += 1;
      this.scrollOperations.push("bottom");
      this.buffer.active.viewportY = this.buffer.active.baseY;
    }
    scrollToLine(line: number): void {
      this.assertLive();
      this.scrollOperations.push(`line:${line}`);
      this.buffer.active.viewportY = Math.max(0, Math.min(this.buffer.active.baseY, line));
    }
    scrollLines(amount: number): void {
      this.assertLive();
      this.scrollOperations.push(`lines:${amount}`);
      this.buffer.active.viewportY = Math.max(
        0,
        Math.min(this.buffer.active.baseY, this.buffer.active.viewportY + amount),
      );
    }
    scrollPages(amount: number): void {
      this.assertLive();
      this.scrollOperations.push(`pages:${amount}`);
      this.buffer.active.viewportY = Math.max(
        0,
        Math.min(this.buffer.active.baseY, this.buffer.active.viewportY + amount * this.rows),
      );
    }
    paste(): void {
      this.assertLive();
    }
    hasSelection(): boolean {
      this.assertLive();
      return this.selection;
    }
    getSelection(): string {
      this.assertLive();
      return "";
    }
    attachCustomKeyEventHandler(): void {
      this.assertLive();
    }
    onData(): { dispose: () => void } {
      this.assertLive();
      return { dispose: () => {} };
    }
    onResize(listener: (size: { cols: number; rows: number }) => void): {
      dispose: () => void;
    } {
      this.assertLive();
      this.resizeListener = listener;
      return {
        dispose: () => {
          if (this.resizeListener === listener) {
            this.resizeListener = null;
          }
        },
      };
    }
    resize(cols: number, rows: number): void {
      this.assertLive();
      this.resizeAttempts.push({ cols, rows });
      if (this.cols === cols && this.rows === rows) {
        return;
      }
      this.cols = cols;
      this.rows = rows;
      const size = { cols, rows };
      this.resizes.push(size);
      this.resizeListenerEvents.push(size);
      this.resizeListener?.(size);
    }

    private assertLive(): void {
      if (this.disposed) {
        throw new Error("terminal disposed");
      }
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
      const terminal = this.terminal;
      if (!terminal) return;
      terminal.ordinaryFitCalls += 1;
      if (terminal.cols === fitViewport.cols && terminal.rows === fitViewport.rows) {
        return;
      }
      terminal.resize(fitViewport.cols, fitViewport.rows);
    });
    proposeDimensions(): { cols: number; rows: number } | undefined {
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
const GONE = [71, 79, 78, 69]; // "GONE" — produced while detached
const LIVE = [76, 73, 86, 69]; // "LIVE"

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((next, fail) => {
    resolve = next;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function setTerminalContainerWidth(element: HTMLElement, width: number): void {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () =>
      ({
        x: 0,
        y: 0,
        top: 0,
        left: 0,
        right: width,
        bottom: 600,
        width,
        height: 600,
        toJSON: () => ({}),
      }) as DOMRect,
  });
}

function pulseResult(
  request: MainTerminalLayoutPulseRequest,
  status: MainTerminalLayoutPulseStatus,
  reason: MainTerminalLayoutPulseReason,
): MainTerminalLayoutPulseResult {
  const emptyPhase = {
    sidebarWidth: null,
    hostWidth: null,
    cols: null,
    rows: null,
    baselineObservedEpoch: null,
    completedObserverAck: null,
  };
  return {
    status,
    reason,
    trace: {
      version: 1,
      requestId: request.requestId,
      sessionId: request.sessionId,
      attachGeneration: request.attachGeneration,
      status,
      reason,
      original: { ...emptyPhase },
      expanded: { ...emptyPhase },
      restored: { ...emptyPhase },
      dwellMs: status === "completed" ? 200 : 0,
      settingsWritesDelta: 0,
    },
  };
}

function queryTerminalTarget(element: HTMLElement, sessionId = SESSION_A) {
  const result = executeUiTerminalController({
    element,
    sessionId,
    operation: { kind: "query" },
  });
  if (!result || !result.ok) {
    throw new Error(
      `Expected live terminal target, got ${result?.error ?? "controller unavailable"}`,
    );
  }
  return result.target;
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

async function flushFramesUntil(
  frames: ReturnType<typeof installDeterministicAnimationFrames>,
  predicate: () => boolean,
): Promise<void> {
  for (let pass = 0; pass < 30; pass += 1) {
    await flushPromises();
    await frames.flush();
    if (predicate()) {
      return;
    }
  }
  throw new Error("Timed out while driving deterministic animation frames");
}

// Fixture: five viewports of 24 rows of 80-column line bytes (5 * 24 * 80 =
// 9600 bytes), the multi-viewport idle history a Codex lifecycle accumulates.
const MULTIVIEW: number[] = Array.from(
  { length: 5 * 24 * 80 },
  (_, index) => (index * 31) % 256
);

// Fixture: a 64 * 1024 byte ring batch, the near-64-KiB history bound.
const RING_64K: number[] = Array.from(
  { length: 64 * 1024 },
  (_, index) => (index * 17 + 3) % 256
);

// Drains the instance's pending write completions FIFO. A completion may
// synchronously release a drain whose continuation (reset, next queued write)
// lands on a microtask, so callers interleave this with flushPromises.
function completeWriteCallbacks(instance: FakeTerminalInstance): void {
  const callbacks = instance.pendingWriteCallbacks.splice(0);
  for (const callback of callbacks) {
    callback();
  }
}

// Models xterm having parsed `lines` of scrollback: the buffer holds `lines`
// rows, `baseY` sits above a `rows`-tall viewport, and the view shows the top.
function simulateParsedHistory(instance: FakeTerminalInstance, lines: number): void {
  instance.buffer.active.length = lines;
  instance.buffer.active.baseY = lines - instance.rows;
  instance.buffer.active.viewportY = 0;
}

// Models an intentional user scroll-up (wheel listener side effect or direct
// scrollbar drag): the viewport leaves the bottom.
function simulateUserScrollUp(instance: FakeTerminalInstance): void {
  instance.buffer.active.viewportY = 0;
}

function ptyResizeTuples(
  fake: FakeTransport,
  sessionId: string,
): Array<{ cols: number; rows: number }> {
  return fake
    .callsFor("pty_resize")
    .filter((call) => String(call.args.sessionId) === sessionId)
    .map((call) => ({
      cols: Number(call.args.cols),
      rows: Number(call.args.rows),
    }));
}

async function passTwoAnimationFrames(): Promise<void> {
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.resolve("detach_terminal_output", undefined);
  fake.onInvoke("activate_terminal_output", (args) => ({
    sessionId: String(args.sessionId),
    data: SNAP,
    rows: null,
    cols: null,
    sequence: 0,
  }));
}

function instancesFor(sessionId: string): FakeTerminalInstance[] {
  return xterm.instances.filter(
    (candidate) =>
      !candidate.disposed &&
      candidate.element?.getAttribute("data-ac-session-id") === sessionId,
  );
}

function attachedSessionIds(fake: FakeTransport): string[] {
  return fake.callsFor("activate_terminal_output").map((call) => String(call.args.sessionId));
}

function detachedSessionIds(fake: FakeTransport): string[] {
  return fake.callsFor("detach_terminal_output").map((call) => String(call.args.sessionId));
}

describe("TerminalView attachment (#1363)", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanupResizeObserver: (() => void) | null = null;
  let warn: ReturnType<typeof vi.spyOn>;
  let debug: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    cleanupResizeObserver = installControllableResizeObserver();
    resetUiStoresForTests();
    resetUiTerminalControllerForTests();
    xterm.autoCompleteWrites = true;
    for (const instance of xterm.instances) {
      instance.pendingWriteCallbacks.length = 0;
      instance.writeThrows = false;
    }
    xterm.instances.length = 0;
    fitViewport.cols = 80;
    fitViewport.rows = 24;
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    debug = vi.spyOn(console, "debug").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanupResizeObserver?.();
    cleanupResizeObserver = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    resetUiTerminalControllerForTests();
    xterm.instances.length = 0;
    warn.mockRestore();
    debug.mockRestore();
  });

  // A -> B -> A with the detach of A still in flight when the selection comes
  // back. Without the desired-state check after the await, B's transition would
  // resume and attach B — the window would end up attached to a session it is
  // not displaying, and A's stream would stop arriving.
  it("never attaches a superseded target and ends attached to the last selection", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const detachGate = deferred<void>();
    let detachCalls = 0;
    fake.onInvoke("detach_terminal_output", () => {
      detachCalls += 1;
      return detachCalls === 1 ? detachGate.promise : undefined;
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));

      // Switch away: the detach of A is issued and stays in flight.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(detachCalls).toBe(1));

      // Switch back before the detach settles, then let it settle.
      terminalStore.setActiveSessionForTests(SESSION_A);
      detachGate.resolve();
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_A])
      );

      // B was visible for a moment but never attached: its transition was
      // superseded before it could issue one.
      expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_A]);
      expect(detachedSessionIds(fake)).toEqual([SESSION_A]);
    } finally {
      rendered.cleanup();
    }
  });

  // One rejected invoke must cost exactly one transition, not the window.
  it("keeps transitioning after a rejected attach, and owes no detach for it", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    let attachCalls = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      attachCalls += 1;
      if (attachCalls === 1) {
        throw "sessionUnavailable";
      }
      return {
        sessionId: String(args.sessionId),
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 0,
      };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      await flushPromises();
      expect(instancesFor(SESSION_A)[0].writes).toHaveLength(0);

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)[0]?.writes).toHaveLength(1));

      expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B]);
      // Criterion L: a rejected attach left the backend map unchanged, so this
      // window owes no detach for A.
      expect(detachedSessionIds(fake)).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  // Criterion P's frontend half, and the reason it is a SEPARATE assertion from
  // anything about bytes: the backend emits with `emit_to(label, ...)`, but
  // Tauri short-circuits that label filter for a listener registered as
  // `EventTarget::Any` (`tauri-2.10.3/src/event/listener.rs:306-311`), and
  // `Any` is the JS `listen()` default (`@tauri-apps/api/event.js:69-73`). A
  // regression to the default is INVISIBLE downstream — no wrong byte is ever
  // written, because the visibility filter at the single writer drops the
  // foreign session — so the only thing that can catch it is the registration
  // itself. What it costs is the bridge multiplier of plan 7.4: every attached
  // window deserializing every other attached window's flush.
  it("registers the pty_output listener scoped to this window", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(fake.listensFor("pty_output")).toHaveLength(1));

      const [registration] = fake.listensFor("pty_output");
      expect(registration.options?.scopeToCurrentWindow).toBe(true);
      // Spelled out separately: an absent option is the `Any` default, which is
      // exactly the regression this test exists to fail on. The flag becoming
      // a concrete window label is pinned in `transport-tauri.test.ts`.
      expect(registration.options).not.toBeUndefined();
    } finally {
      rendered.cleanup();
    }
  });

  it("executes all visible-terminal operations through public xterm APIs with no PTY effects", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1));
      const terminal = instancesFor(SESSION_A)[0];
      terminal.buffer.active.length = 124;
      terminal.buffer.active.baseY = 100;
      terminal.buffer.active.viewportY = 100;
      terminal.scrollOperations.length = 0;
      const ptyWritesBefore = fake.callsFor("pty_write").length;
      const ptyResizesBefore = fake.callsFor("pty_resize").length;
      const localResizesBefore = terminal.resizes.length;
      const execute = (operation:
        | { kind: "query" | "top" | "bottom" }
        | { kind: "line" | "lines" | "pages"; value: number }) =>
        executeUiTerminalController({
          element: terminal.element!,
          sessionId: SESSION_A,
          operation,
        });

      expect(execute({ kind: "query" })).toMatchObject({
        ok: true,
        target: {
          sessionId: SESSION_A,
          baseY: 100,
          viewportY: 100,
          length: 124,
          cols: 80,
          rows: 24,
          type: "normal",
          atBottom: true,
        },
      });
      expect(execute({ kind: "top" })).toMatchObject({
        ok: true,
        target: { viewportY: 0, atBottom: false },
      });
      expect(execute({ kind: "bottom" })).toMatchObject({
        ok: true,
        target: { viewportY: 100, atBottom: true },
      });
      expect(execute({ kind: "line", value: 9 })).toMatchObject({
        ok: true,
        target: { viewportY: 9, atBottom: false },
      });
      expect(execute({ kind: "lines", value: -4 })).toMatchObject({
        ok: true,
        target: { viewportY: 5, atBottom: false },
      });
      expect(execute({ kind: "pages", value: 1 })).toMatchObject({
        ok: true,
        target: { viewportY: 29, atBottom: false },
      });

      expect(terminal.scrollOperations).toEqual([
        "top",
        "bottom",
        "line:9",
        "lines:-4",
        "pages:1",
      ]);
      expect(fake.callsFor("pty_write")).toHaveLength(ptyWritesBefore);
      expect(fake.callsFor("pty_resize")).toHaveLength(ptyResizesBefore);
      expect(terminal.resizes).toHaveLength(localResizesBefore);
    } finally {
      rendered.cleanup();
    }
  });

  it("ui-terminal top during the settle window marks scroll intent and prevents bottoming", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toEqual([SNAP]));
      const terminal = instancesFor(SESSION_A)[0];
      await passTwoAnimationFrames();
      simulateParsedHistory(terminal, 124);
      terminal.buffer.active.viewportY = terminal.buffer.active.baseY;

      const result = executeUiTerminalController({
        element: terminal.element!,
        sessionId: SESSION_A,
        operation: { kind: "top" },
      });
      expect(result).toMatchObject({ ok: true, target: { viewportY: 0, atBottom: false } });
      const attemptsBefore = terminal.resizeAttempts.length;
      const fitsBefore = terminal.ordinaryFitCalls;

      completeWriteCallbacks(terminal);
      await waitFor(() =>
        expect(
          debug.mock.calls.some((call: unknown[]) =>
            String(call[0]).startsWith(`[terminal] attach ${SESSION_A} settled:`),
          ),
        ).toBe(true),
      );
      expect(terminal.scrollToBottomCalls).toBe(0);
      expect(terminal.buffer.active.viewportY).toBe(0);
      expect(terminal.ordinaryFitCalls).toBe(fitsBefore + 1);
      expect(terminal.resizeAttempts.slice(attemptsBefore)).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  it("ui-terminal query during the settle window is read-only and does not suppress bottoming", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toEqual([SNAP]));
      const terminal = instancesFor(SESSION_A)[0];
      simulateParsedHistory(terminal, 124);
      terminal.buffer.active.viewportY = 0;
      expect(
        executeUiTerminalController({
          element: terminal.element!,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: true, target: { viewportY: 0, atBottom: false } });

      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);
    } finally {
      rendered.cleanup();
    }
  });

  it("fails closed for hidden, mismatched, replaced, disconnected, and destroyed terminal entries", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1));
      const terminalA = instancesFor(SESSION_A)[0];
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1));
      const terminalB = instancesFor(SESSION_B)[0];
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A]));

      expect(
        executeUiTerminalController({
          element: terminalB.element!,
          sessionId: SESSION_B,
          operation: { kind: "top" },
        }),
      ).toMatchObject({ ok: false, error: "terminal_session_not_visible" });
      expect(terminalB.scrollOperations).not.toContain("top");

      terminalA.element!.setAttribute("data-ac-session-id", SESSION_B);
      expect(
        executeUiTerminalController({
          element: terminalA.element!,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: false, error: "terminal_target_mismatch" });
      terminalA.element!.setAttribute("data-ac-session-id", SESSION_A);

      const replacement = document.createElement("div");
      replacement.setAttribute("data-ac-testid", `terminal.session.${SESSION_A}`);
      replacement.setAttribute("data-ac-session-id", SESSION_A);
      document.body.append(replacement);
      expect(
        executeUiTerminalController({
          element: replacement,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: false, error: "terminal_target_mismatch" });

      const oldElement = terminalA.element!;
      oldElement.remove();
      expect(
        executeUiTerminalController({
          element: oldElement,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: false, error: "terminal_entry_stale" });
      rendered.root.append(oldElement);

      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      expect(terminalA.disposed).toBe(true);
      expect(
        executeUiTerminalController({
          element: oldElement,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: false, error: "terminal_entry_stale" });
    } finally {
      rendered.cleanup();
    }
  });

  it("unregisters the controller on unmount and a remount owns the replacement", async () => {
    const fakeA = new FakeTransport();
    setupTransport(fakeA);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const first = renderWithFakeTransport(() => <TerminalView />, fakeA);
    await waitFor(() => expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1));
    const oldElement = instancesFor(SESSION_A)[0].element!;
    first.cleanup();
    expect(
      executeUiTerminalController({
        element: oldElement,
        sessionId: SESSION_A,
        operation: { kind: "query" },
      }),
    ).toBeNull();

    resetUiStoresForTests();
    terminalStore.setActiveSessionForTests(SESSION_A);
    const fakeB = new FakeTransport();
    setupTransport(fakeB);
    const second = renderWithFakeTransport(() => <TerminalView />, fakeB);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1));
      const current = instancesFor(SESSION_A)[0];
      first.cleanup();
      expect(
        executeUiTerminalController({
          element: current.element!,
          sessionId: SESSION_A,
          operation: { kind: "query" },
        }),
      ).toMatchObject({ ok: true, target: { sessionId: SESSION_A } });
    } finally {
      second.cleanup();
    }
    expect(
      executeUiTerminalController({
        element: oldElement,
        sessionId: SESSION_A,
        operation: { kind: "query" },
      }),
    ).toBeNull();
  });

  // ORDERING, not outcome: the attach must not reach the backend until this
  // window is listening. Issuing the `listen` first does not order the two —
  // `plugin:event|listen` is an async Tauri command on the async runtime while
  // `activate_terminal_output` is sync on the main thread — so the gate is what
  // holds. A chunk lost in that window is silent and permanent: its sequence is
  // above the seed's, so the watermark never replays it.
  it("does not attach until the pty_output listener has registered", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    // Hold the registration open. Nothing is recorded as listening until the
    // gate releases, which is exactly the state the invariant is about.
    const listenGate = deferred<void>();
    const realListen = fake.listen.bind(fake);
    fake.listen = (async (event: string, callback, options) => {
      if (event === "pty_output") {
        await listenGate.promise;
      }
      return realListen(event, callback, options);
    }) as typeof fake.listen;

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      // The terminal is built and visible; only the attach is withheld.
      await waitFor(() => expect(instancesFor(SESSION_A)).toHaveLength(1));
      await flushPromises();
      await flushPromises();
      expect(fake.listensFor("pty_output")).toHaveLength(0);
      expect(attachedSessionIds(fake)).toEqual([]);

      listenGate.resolve();

      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      expect(fake.listensFor("pty_output")).toHaveLength(1);

      // The other half of criterion C: a chunk emitted at the first
      // opportunity after the attach still lands. Whether it arrives before or
      // after the seed settles, it ends on screen exactly once — retained and
      // replayed past the seed's sequence in the first case, written straight
      // through in the second.
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });
      await waitFor(() =>
        expect(instancesFor(SESSION_A)[0].screen).toEqual([SNAP, LIVE])
      );
    } finally {
      rendered.cleanup();
    }
  });

  // The gate must fail CLOSED. A window whose listener never registered cannot
  // render the stream, so attaching would only ask the backend to emit into
  // nothing — and it must still not poison the chain for anything else.
  it("leaves the window unattached when the listener registration fails", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const realListen = fake.listen.bind(fake);
    fake.listen = (async (event: string, callback, options) => {
      if (event === "pty_output") {
        throw new Error("listen rejected");
      }
      return realListen(event, callback, options);
    }) as typeof fake.listen;

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)).toHaveLength(1));
      await flushPromises();
      await flushPromises();

      expect(attachedSessionIds(fake)).toEqual([]);
      // Not poisoned, and nothing was attached, so nothing is owed back.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)).toHaveLength(1));
      await flushPromises();
      expect(attachedSessionIds(fake)).toEqual([]);
      expect(detachedSessionIds(fake)).toEqual([]);
    } finally {
      rendered.cleanup();
    }
  });

  // Criterion L, frontend half. An unavailable parser still attaches — refusing
  // would leave the terminal black for the life of the session (#955) — and it
  // emits chunks with NO sequence. Such a chunk is written live on arrival, and
  // the attach settling without a snapshot must not write it a second time:
  // there is no reset on that path, so there is nothing to replay.
  it("writes an unsequenced chunk exactly once when the attach returns no snapshot", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const attach = deferred<null>();
    fake.onInvoke("activate_terminal_output", () => attach.promise);

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Unsequenced: the backend's parser is unavailable for this session.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE });
      await waitFor(() => expect(terminal.writes).toEqual([LIVE]));
      await passTwoAnimationFrames();
      const attemptsBefore = terminal.resizeAttempts.length;
      const fitsBefore = terminal.ordinaryFitCalls;

      attach.resolve(null);
      await flushPromises();
      await passTwoAnimationFrames();

      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.resets).toBe(0);
      expect(terminal.resizeAttempts.slice(attemptsBefore)).toEqual([]);
      expect(terminal.ordinaryFitCalls).toBeGreaterThan(fitsBefore);
    } finally {
      rendered.cleanup();
    }
  });

  // Plan 3.4.2: while detached, the backend keeps parsing and keeps filling the
  // history ring but emits nothing. Re-seeding on every attach is what makes
  // that output appear instead of being hidden under a resumed live stream.
  it("shows the output produced while detached, with no gap and no duplicated block", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    let attachesOfA = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId !== SESSION_A) {
        return { sessionId, data: [], rows: null, cols: null, sequence: 0 };
      }
      attachesOfA += 1;
      return attachesOfA === 1
        ? { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 }
        : // The ring now also holds everything A produced while detached.
          { sessionId, data: [...SNAP, ...GONE], rows: null, cols: null, sequence: 5 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(detachedSessionIds(fake)).toEqual([SESSION_A]));
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B])
      );

      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() => expect(terminal.writes).toHaveLength(2));

      // The visible screen is the re-seed alone: SNAP appears exactly once,
      // and GONE — produced while this window was detached — is there.
      expect(terminal.screen).toEqual([[...SNAP, ...GONE]]);
      expect(terminal.resets).toBe(2); // one per attach: every attach re-seeds
      expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A]);
    } finally {
      rendered.cleanup();
    }
  });

  // Two mounted views, each with its own registry, its own attachment and its
  // own write filter — the production shape, one TerminalView per window.
  //
  // What this CANNOT express: two DIFFERENT visible sessions. `terminalStore`
  // is a module singleton and every view reads `activeSessionId` from it, so
  // in one jsdom process both views always show the same session; in
  // production each window is a separate WebView with its own module instance.
  // Criterion A's "two windows on two different sessions" therefore lives in
  // the backend attached-set tests. What is real here is criterion B (two
  // views on one session both render the stream, each with its own attachment)
  // and criterion G (no hidden or unknown session ever reaches Terminal.write).
  it("gives each mounted view its own attachment and writes only the visible session", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(
      () => (
        <div>
          <TerminalView />
          <TerminalView />
        </div>
      ),
      fake,
    );
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)).toHaveLength(2));
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_A])
      );

      // A session neither view has ever shown is written by neither.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_B, data: LIVE, sequence: 1 });
      await flushPromises();
      for (const instance of instancesFor(SESSION_A)) {
        expect(instance.writes).toEqual([SNAP]);
      }

      // The visible session reaches BOTH views (criterion B).
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 1 });
      await flushPromises();
      for (const instance of instancesFor(SESSION_A)) {
        expect(instance.writes).toEqual([SNAP, LIVE]);
      }

      // Switch both views to B; A stays retained but hidden in both registries.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)).toHaveLength(2));
      await waitFor(() =>
        expect(detachedSessionIds(fake)).toEqual([SESSION_A, SESSION_A])
      );

      // A retained-but-hidden terminal never receives a write (criterion G).
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: GONE, sequence: 2 });
      await flushPromises();
      for (const instance of instancesFor(SESSION_A)) {
        expect(instance.writes).toEqual([SNAP, LIVE]);
      }
      await waitFor(() => {
        for (const instance of instancesFor(SESSION_B)) {
          expect(instance.writes).toEqual([SNAP]);
        }
      });
    } finally {
      rendered.cleanup();
    }
  });

  // #1439: a re-attach can settle with NO snapshot (the backend grid reconcile
  // refused the seed) while the PTY sits at the grid the other window drove it
  // to. The attach settle must invalidate the viewport dedup key and re-impose
  // this window's grid: the embedded box did not change across the detach, so
  // without the invalidation `sendPtyResize` compares the refit against the
  // key primed before the detach, finds them equal, and never invokes
  // `pty_resize`, leaving live bytes to garble this xterm indefinitely.
  it("a re-attach that resolves without a snapshot resyncs the viewport before live writes land", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    // The fit dims (80x24, pinned by the FakeTerminal and FitAddon mocks) must
    // differ from the entry's spawnViewport, or the FIRST sync would dedup at
    // creation time and the priming assert below would be vacuous.
    rememberSpawnViewport(SESSION_A, { cols: 100, rows: 30 });
    let attachesOfA = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId !== SESSION_A) {
        return { sessionId, data: [], rows: null, cols: null, sequence: 0 };
      }
      attachesOfA += 1;
      return attachesOfA === 1
        ? { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 }
        : // The #1439 reconcile-miss outcome: no seed, parser still Available,
          // live events still sequenced.
          null;
    });
    const resizesOfA = () =>
      fake
        .callsFor("pty_resize")
        .filter((call) => String(call.args.sessionId) === SESSION_A)
        .map((call) => ({
          cols: Number(call.args.cols),
          rows: Number(call.args.rows),
        }));

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];

      // The priming sync, pre-seed imposition, and the
      // {second priming frame, settle} pair each contribute one send in the
      // harness ordering. All carry this window's fitted grid.
      await waitFor(() =>
        expect(resizesOfA()).toEqual([
          { cols: 80, rows: 24 },
          { cols: 80, rows: 24 },
          { cols: 80, rows: 24 },
        ])
      );

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(detachedSessionIds(fake)).toEqual([SESSION_A]));
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B])
      );

      // Segment the invoke log: only re-attach calls count from here on.
      const resizesBeforeReattach = resizesOfA().length;

      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );

      // The pre-seed imposition and the {second priming frame, seedless
      // resync} pair each re-impose this window's grid. Without the dedup-key
      // invalidation the refit equals the primed key and these sends disappear:
      // the incident geometry returns, and this wait times out.
      await waitFor(() =>
        expect(resizesOfA().slice(resizesBeforeReattach)).toEqual([
          { cols: 80, rows: 24 },
          { cols: 80, rows: 24 },
        ])
      );

      // Ordering: the resync has landed and the live write has not; only now
      // is the live chunk delivered (harness-controlled ordering, plan 9.2).
      expect(terminal.writes).toEqual([SNAP]);
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 5,
      });
      await waitFor(() => expect(terminal.writes).toEqual([SNAP, LIVE]));

      // The no-snapshot contract held: no reset during the re-attach, and the
      // pre-detach buffer content survived the whole cycle.
      expect(terminal.resets).toBe(1);
      expect(terminal.screen).toEqual([SNAP, LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 - the attach replay must be ordered: every retained live byte was
  // written on arrival and queued BEFORE the reset+snapshot, so the snapshot
  // write must have parsed completely before the retained replay is written.
  // The snapshot carries sequence 1 (pinned below LIVE's 5: with sequence >= 5
  // the replay watermark would drop LIVE and the pre-fix red assertion would
  // not occur). PRE-FIX RED: screen == [LIVE, SNAP, LIVE] (the straddled,
  // duplicated replay).
  it("waits for the snapshot write to parse before replaying retained live output", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    const attachGate = deferred<PtyScreenSnapshot | null>();
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        return attachGate.promise;
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Retained live output while the seed is in flight: queued, not applied.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 });
      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.screen).toEqual([]);

      attachGate.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 1,
      });
      await flushPromises();

      // The drain holds: the live byte is still queued, the snapshot is not
      // yet written, nothing is on screen.
      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.screen).toEqual([]);

      // Complete the live write: the drain releases, the reset runs, and the
      // snapshot write is queued — before its callback completes.
      completeWriteCallbacks(terminal);
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE, SNAP]);

      // Complete the snapshot write: the replay write is queued.
      completeWriteCallbacks(terminal);
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE, SNAP, LIVE]);

      // Complete the replay write: snapshot bytes then live bytes, exactly
      // once.
      completeWriteCallbacks(terminal);
      await flushPromises();
      expect(terminal.screen).toEqual([SNAP, LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 - after replay and the fitted resize, the attach must bottom the
  // viewport to the current screen exactly once. The attach fetch is held so
  // the selectSession priming sync fires with the PRE-snapshot grid (80x24),
  // exactly as in production where the attach IPC round-trip spans animation
  // frames; the snapshot carries rows 27 / cols 81, so after the #1439 dedup
  // key invalidation the settle's own resize is NOT deduplicated. pty_resize
  // is gated so the history simulation provably lands after the reset (the
  // reset zeroes the buffer metrics) and before the bottoming. PRE-FIX RED:
  // no settle at all — scrollToBottomCalls stays 0 and viewportY (0) <
  // baseY (108).
  it("settles the viewport to the current screen exactly once after replay and fit", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const attachGate = deferred<PtyScreenSnapshot | null>();
    const resizeGate = deferred<void>();
    fake.onInvoke("pty_resize", (args) => {
      if (Number(args.cols) === 81 && Number(args.rows) === 27) {
        return resizeGate.promise;
      }
      return undefined;
    });
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        return attachGate.promise;
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });
    const resizesOfA = () =>
      fake
        .callsFor("pty_resize")
        .filter((call) => String(call.args.sessionId) === SESSION_A)
        .map((call) => ({
          cols: Number(call.args.cols),
          rows: Number(call.args.rows),
        }));

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // The priming sync and the awaited pre-seed imposition carry the
      // pre-snapshot grid. The second sync frame deduplicates against the key
      // re-primed by the pre-seed send.
      await waitFor(() =>
        expect(resizesOfA()).toEqual([
          { cols: 80, rows: 24 },
          { cols: 80, rows: 24 },
        ])
      );
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      expect(resizesOfA()).toEqual([
        { cols: 80, rows: 24 },
        { cols: 80, rows: 24 },
      ]);

      fitViewport.cols = 81;
      fitViewport.rows = 27;
      attachGate.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: 27,
        cols: 81,
        sequence: 0,
      });

      // The settle's pty_resize carries the snapshot grid: proof the replay
      // and the reset ran (the reset zeroes the buffer metrics, so the
      // history simulation must come after it).
      await waitFor(() =>
        expect(resizesOfA()).toContainEqual({ cols: 81, rows: 27 })
      );
      simulateParsedHistory(terminal, 135);

      // No bottoming before the resize outcome.
      expect(terminal.scrollToBottomCalls).toBe(0);

      resizeGate.resolve();
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      expect(terminal.buffer.active.viewportY).toBe(108);
      expect(terminal.buffer.active.baseY).toBe(108);

      // The single settled record carries the full evidence set: viewport
      // metrics, terminal grid, snapshot grid, seed size, buffer type.
      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .find((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled).toContain("viewportY=108");
      expect(settled).toContain("baseY=108");
      expect(settled).toContain("bufferLength=135");
      expect(settled).toContain("cols=81");
      expect(settled).toContain("rows=27");
      expect(settled).toContain("type=normal");
      expect(settled).toContain("snapshotCols=81");
      expect(settled).toContain("snapshotRows=27");
      expect(settled).toContain("seedBytes=4");
      expect(settled).toContain("resize=sent");
    } finally {
      rendered.cleanup();
    }
  });

  // #1532 replaces the falsified direct xterm round-trip with one real,
  // App-owned split-layout pulse. The observer/settlement cases below use a
  // controllable ResizeObserver and the shared deterministic frame queue.

  it("dispatches one non-empty request and publishes the bounded unhandled trace after settlement", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const capture = (event: Event) => {
      requests.push(
        (event as CustomEvent<MainTerminalLayoutPulseRequest>).detail,
      );
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, capture);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1));
      expect(requests).toHaveLength(1);
      expect(requests[0].accepted).toBe(false);
      expect(queryTerminalTarget(instancesFor(SESSION_A)[0].element!)).toMatchObject({
        layoutPulse: {
          version: 1,
          requestId: requests[0].requestId,
          sessionId: SESSION_A,
          attachGeneration: requests[0].attachGeneration,
          status: "skipped",
          reason: "unhandled",
        },
      });
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, capture);
    }
  });

  it.each([
    ["completed", "completed"],
    ["skipped", "busy"],
    ["cancelled", "stale"],
    ["failed", "request_timeout"],
  ] as const)(
    "resumes ordinary final fit/send after a %s/%s pulse result",
    async (status, reason) => {
      const fake = new FakeTransport();
      setupTransport(fake);
      const requests: MainTerminalLayoutPulseRequest[] = [];
      const respond = (event: Event) => {
        const request = (
          event as CustomEvent<MainTerminalLayoutPulseRequest>
        ).detail;
        request.accepted = true;
        requests.push(request);
        request.complete(pulseResult(request, status, reason));
        request.complete(pulseResult(request, "failed", "exception"));
      };
      window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
      terminalStore.setActiveSessionForTests(SESSION_A);
      const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
      try {
        await waitFor(() =>
          expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1),
        );
        expect(requests).toHaveLength(1);
        expect(ptyResizeTuples(fake, SESSION_A).length).toBeGreaterThan(0);
        expect(queryTerminalTarget(instancesFor(SESSION_A)[0].element!).layoutPulse).toMatchObject({
          status,
          reason,
          attachGeneration: requests[0].attachGeneration,
        });
      } finally {
        rendered.cleanup();
        window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
      }
    },
  );

  it("preserves user scroll intent while the accepted layout pulse is pending", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const hold = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      requests.push(request);
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(requests).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];
      simulateParsedHistory(terminal, 120);
      expect(
        executeUiTerminalController({
          element: terminal.element!,
          sessionId: SESSION_A,
          operation: { kind: "top" },
        }),
      ).toMatchObject({ ok: true, target: { viewportY: 0, atBottom: false } });

      requests[0].complete(pulseResult(requests[0], "completed", "completed"));
      await waitFor(() =>
        expect(
          debug.mock.calls.some((call: unknown[]) =>
            String(call[0]).startsWith(`[terminal] attach ${SESSION_A} settled:`),
          ),
        ).toBe(true),
      );
      expect(terminal.scrollToBottomCalls).toBe(0);
      expect(terminal.buffer.active.viewportY).toBe(0);
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    }
  });

  it("records delivered epochs immediately and only publishes two-fit same-entry acknowledgements", async () => {
    const frames = installDeterministicAnimationFrames();
    const fake = new FakeTransport();
    setupTransport(fake);
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const hold = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      requests.push(request);
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await flushFramesUntil(frames, () => requests.length === 1);
      const request = requests[0];
      const terminal = instancesFor(SESSION_A)[0];
      setTerminalContainerWidth(terminal.element!, 800);

      expect(request.sample()).toMatchObject({
        hostWidth: 800,
        cols: 80,
        rows: 24,
        observedObserverEpoch: 0,
        completedObserverAck: null,
      });

      resizeObserverControl.latest().deliver();
      expect(request.sample()).toMatchObject({
        observedObserverEpoch: 1,
        completedObserverAck: null,
      });
      expect(await frames.flushFrame()).toBe(true);
      expect(request.sample()?.completedObserverAck).toBeNull();

      setTerminalContainerWidth(terminal.element!, 816);
      fitViewport.cols = 82;
      expect(await frames.flushFrame()).toBe(true);
      expect(request.sample()?.completedObserverAck).toEqual({
        epoch: 1,
        first: { hostWidth: 800, cols: 80, rows: 24 },
        second: { hostWidth: 816, cols: 82, rows: 24 },
      });

      resizeObserverControl.latest().deliver();
      await frames.flush();
      expect(request.sample()?.completedObserverAck).toEqual({
        epoch: 2,
        first: { hostWidth: 816, cols: 82, rows: 24 },
        second: { hostWidth: 816, cols: 82, rows: 24 },
      });

      resizeObserverControl.latest().deliver();
      expect(await frames.flushFrame()).toBe(true);
      setTerminalContainerWidth(terminal.element!, 800);
      fitViewport.cols = 80;
      expect(await frames.flushFrame()).toBe(true);
      expect(request.sample()?.completedObserverAck).toEqual({
        epoch: 3,
        first: { hostWidth: 816, cols: 82, rows: 24 },
        second: { hostWidth: 800, cols: 80, rows: 24 },
      });

      resizeObserverControl.latest().deliver();
      await frames.flush();
      expect(request.sample()?.completedObserverAck).toEqual({
        epoch: 4,
        first: { hostWidth: 800, cols: 80, rows: 24 },
        second: { hostWidth: 800, cols: 80, rows: 24 },
      });
      expect(ptyResizeTuples(fake, SESSION_A)).toEqual(
        expect.arrayContaining([
          { cols: 82, rows: 24 },
          { cols: 80, rows: 24 },
        ]),
      );

      const beforeDirectControl = request.sample();
      terminal.resize(80, 24);
      expect(request.sample()).toEqual(beforeDirectControl);

      request.complete(pulseResult(request, "completed", "completed"));
      await flushPromises();
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
      frames.restore();
    }
  });

  it.each(["hidden", "disconnected", "visibility", "destroyed", "teardown"] as const)(
    "abandons an observer record after %s invalidates the captured entry",
    async (scenario) => {
      const frames = installDeterministicAnimationFrames();
      const fake = new FakeTransport();
      setupTransport(fake);
      const requests: MainTerminalLayoutPulseRequest[] = [];
      const hold = (event: Event) => {
        const request = (
          event as CustomEvent<MainTerminalLayoutPulseRequest>
        ).detail;
        request.accepted = true;
        requests.push(request);
      };
      window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
      terminalStore.setActiveSessionForTests(SESSION_A);
      const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
      let cleaned = false;
      try {
        await flushFramesUntil(frames, () => requests.length === 1);
        const request = requests[0];
        const terminal = instancesFor(SESSION_A)[0];
        setTerminalContainerWidth(terminal.element!, 800);
        resizeObserverControl.latest().deliver();
        expect(await frames.flushFrame()).toBe(true);
        expect(request.sample()?.completedObserverAck).toBeNull();

        if (scenario === "hidden") {
          terminal.element!.hidden = true;
        } else if (scenario === "disconnected") {
          terminal.element!.remove();
        } else if (scenario === "visibility") {
          terminalStore.setActiveSessionForTests(SESSION_B);
        } else if (scenario === "destroyed") {
          fake.emitFromBackend("session_destroyed", { id: SESSION_A });
        } else {
          rendered.cleanup();
          cleaned = true;
        }

        await frames.flushFrame();
        expect(request.sample()).toBeNull();
      } finally {
        if (!cleaned) rendered.cleanup();
        window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
        frames.restore();
      }
    },
  );

  it("joins the restored observer resize and publishes its trace only after transport success", async () => {
    const frames = installDeterministicAnimationFrames();
    const fake = new FakeTransport();
    setupTransport(fake);
    const restoredResize = deferred<void>();
    let holdRestoredResize = false;
    let heldRestoredResize = false;
    fake.onInvoke("pty_resize", (args) => {
      if (
        holdRestoredResize &&
        !heldRestoredResize &&
        Number(args.cols) === 80 &&
        Number(args.rows) === 24
      ) {
        heldRestoredResize = true;
        return restoredResize.promise;
      }
      return undefined;
    });
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const hold = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      requests.push(request);
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await flushFramesUntil(frames, () => requests.length === 1);
      const request = requests[0];
      const terminal = instancesFor(SESSION_A)[0];
      setTerminalContainerWidth(terminal.element!, 800);
      holdRestoredResize = true;

      setTerminalContainerWidth(terminal.element!, 816);
      fitViewport.cols = 82;
      resizeObserverControl.latest().deliver();
      await frames.flush();

      setTerminalContainerWidth(terminal.element!, 800);
      fitViewport.cols = 80;
      resizeObserverControl.latest().deliver();
      await frames.flush();
      expect(heldRestoredResize).toBe(true);

      const restoredCallsBeforeSettle = ptyResizeTuples(fake, SESSION_A).filter(
        ({ cols, rows }) => cols === 80 && rows === 24,
      ).length;
      request.complete(pulseResult(request, "completed", "completed"));
      await flushPromises();

      expect(terminal.scrollToBottomCalls).toBe(0);
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");
      expect(
        ptyResizeTuples(fake, SESSION_A).filter(
          ({ cols, rows }) => cols === 80 && rows === 24,
        ),
      ).toHaveLength(restoredCallsBeforeSettle);

      restoredResize.resolve();
      await flushPromises();
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(queryTerminalTarget(terminal.element!).layoutPulse).toEqual(
        pulseResult(request, "completed", "completed").trace,
      );
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
      frames.restore();
    }
  });

  it("keeps a joined failure trace-free before, during, and after its ordinary retry", async () => {
    const frames = installDeterministicAnimationFrames();
    const fake = new FakeTransport();
    setupTransport(fake);
    const restoredResize = deferred<void>();
    let holdRestoredResize = false;
    let heldRestoredResize = false;
    fake.onInvoke("pty_resize", (args) => {
      if (
        holdRestoredResize &&
        !heldRestoredResize &&
        Number(args.cols) === 80 &&
        Number(args.rows) === 24
      ) {
        heldRestoredResize = true;
        return restoredResize.promise;
      }
      return undefined;
    });
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const hold = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      requests.push(request);
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await flushFramesUntil(frames, () => requests.length === 1);
      const request = requests[0];
      const terminal = instancesFor(SESSION_A)[0];
      setTerminalContainerWidth(terminal.element!, 800);
      holdRestoredResize = true;

      setTerminalContainerWidth(terminal.element!, 816);
      fitViewport.cols = 82;
      resizeObserverControl.latest().deliver();
      await frames.flush();
      setTerminalContainerWidth(terminal.element!, 800);
      fitViewport.cols = 80;
      resizeObserverControl.latest().deliver();
      await frames.flush();

      request.complete(pulseResult(request, "completed", "completed"));
      await flushPromises();
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");

      const callsBeforeFailure = ptyResizeTuples(fake, SESSION_A).length;
      restoredResize.reject(new Error("restored resize failed"));
      await flushPromises();
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");

      await waitFor(() =>
        expect(ptyResizeTuples(fake, SESSION_A).length).toBeGreaterThan(callsBeforeFailure),
      );
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
      frames.restore();
    }
  });

  it("bypasses a cached tuple for direct final confirmation and never publishes after failure/retry", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    let failFinalConfirmation = false;
    fake.onInvoke("pty_resize", (args) => {
      if (
        failFinalConfirmation &&
        Number(args.cols) === 80 &&
        Number(args.rows) === 24
      ) {
        failFinalConfirmation = false;
        throw new Error("direct final resize failed");
      }
      return undefined;
    });
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const hold = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      requests.push(request);
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(requests).toHaveLength(1));
      await flushPromises();
      const terminal = instancesFor(SESSION_A)[0];
      const callsBeforeFinal = ptyResizeTuples(fake, SESSION_A).length;
      failFinalConfirmation = true;
      requests[0].complete(pulseResult(requests[0], "completed", "completed"));

      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(ptyResizeTuples(fake, SESSION_A)).toHaveLength(callsBeforeFinal + 1);
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");

      await new Promise((resolve) => setTimeout(resolve, 150));
      expect(ptyResizeTuples(fake, SESSION_A)).toHaveLength(callsBeforeFinal + 1);
      expect(queryTerminalTarget(terminal.element!)).not.toHaveProperty("layoutPulse");
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, hold);
    }
  });

  it.each(["null", "empty", "rejected", "replay failure"] as const)(
    "invalidates the prior generation before a pending seed and keeps %s re-attach trace-free",
    async (outcome) => {
      const fake = new FakeTransport();
      setupTransport(fake);
      const reattach = deferred<PtyScreenSnapshot | null>();
      let aAttachments = 0;
      fake.onInvoke("activate_terminal_output", (args) => {
        const sessionId = String(args.sessionId);
        if (sessionId !== SESSION_A) {
          return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
        }
        aAttachments += 1;
        if (aAttachments === 1) {
          return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
        }
        return reattach.promise;
      });
      const respond = (event: Event) => {
        const request = (
          event as CustomEvent<MainTerminalLayoutPulseRequest>
        ).detail;
        request.accepted = true;
        request.complete(pulseResult(request, "completed", "completed"));
      };
      window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
      terminalStore.setActiveSessionForTests(SESSION_A);
      const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
      try {
        await waitFor(() =>
          expect(queryTerminalTarget(instancesFor(SESSION_A)[0].element!).layoutPulse).toMatchObject({
            attachGeneration: 1,
          }),
        );
        const terminalA = instancesFor(SESSION_A)[0];

        terminalStore.setActiveSessionForTests(SESSION_B);
        await waitFor(() => expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1));
        terminalStore.setActiveSessionForTests(SESSION_A);
        await waitFor(() => expect(aAttachments).toBe(2));

        // `beginSeed` deleted generation 1 before attachOutput could settle.
        expect(queryTerminalTarget(terminalA.element!)).not.toHaveProperty("layoutPulse");

        if (outcome === "null") {
          reattach.resolve(null);
        } else if (outcome === "empty") {
          reattach.resolve({
            sessionId: SESSION_A,
            data: [],
            rows: 24,
            cols: 80,
            sequence: 1,
          });
        } else if (outcome === "rejected") {
          reattach.reject(new Error("snapshot fetch failed"));
        } else {
          terminalA.writeThrows = true;
          reattach.resolve({
            sessionId: SESSION_A,
            data: SNAP,
            rows: 24,
            cols: 80,
            sequence: 1,
          });
        }
        await flushPromises();
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(queryTerminalTarget(terminalA.element!)).not.toHaveProperty("layoutPulse");
        terminalA.writeThrows = false;
      } finally {
        rendered.cleanup();
        window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
      }
    },
  );

  it("omits the new generation trace while snapshot parsing is pending and publishes only after settlement", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const reattach = deferred<PtyScreenSnapshot | null>();
    let aAttachments = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId !== SESSION_A) {
        return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
      }
      aAttachments += 1;
      return aAttachments === 1
        ? { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 }
        : reattach.promise;
    });
    const respond = (event: Event) => {
      const request = (
        event as CustomEvent<MainTerminalLayoutPulseRequest>
      ).detail;
      request.accepted = true;
      request.complete(pulseResult(request, "completed", "completed"));
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() =>
        expect(queryTerminalTarget(instancesFor(SESSION_A)[0].element!).layoutPulse).toMatchObject({
          attachGeneration: 1,
        }),
      );
      const terminalA = instancesFor(SESSION_A)[0];
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1));

      xterm.autoCompleteWrites = false;
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() => expect(aAttachments).toBe(2));
      reattach.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: 24,
        cols: 80,
        sequence: 1,
      });
      await waitFor(() => expect(terminalA.pendingWriteCallbacks.length).toBeGreaterThan(0));
      expect(queryTerminalTarget(terminalA.element!)).not.toHaveProperty("layoutPulse");

      completeWriteCallbacks(terminalA);
      xterm.autoCompleteWrites = true;
      await flushPromises();
      await waitFor(() =>
        expect(queryTerminalTarget(terminalA.element!).layoutPulse).toMatchObject({
          attachGeneration: 2,
        }),
      );
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, respond);
    }
  });

  it("keeps an empty snapshot on the ordinary viewport-sync path without an adjacent-column pulse", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const requests: MainTerminalLayoutPulseRequest[] = [];
    const capture = (event: Event) => {
      requests.push(
        (event as CustomEvent<MainTerminalLayoutPulseRequest>).detail,
      );
    };
    window.addEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, capture);
    fake.onInvoke("activate_terminal_output", (args) => ({
      sessionId: String(args.sessionId),
      data: [],
      rows: 24,
      cols: 80,
      sequence: 0,
    }));
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];
      await passTwoAnimationFrames();
      expect(terminal.writes).toEqual([]);
      expect(terminal.resets).toBe(0);
      expect(terminal.ordinaryFitCalls).toBeGreaterThan(0);
      expect(terminal.resizeAttempts).not.toContainEqual({ cols: 70, rows: 24 });
      expect(requests).toEqual([]);
    } finally {
      rendered.cleanup();
      window.removeEventListener(MAIN_TERMINAL_LAYOUT_PULSE_REQUEST_EVENT, capture);
    }
  });

  it("sends the fitted container grid to the PTY before requesting the seed snapshot", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fitViewport.cols = 100;
    fitViewport.rows = 30;
    fake.onInvoke("activate_terminal_output", (args) => ({
      sessionId: String(args.sessionId),
      data: SNAP,
      rows: 30,
      cols: 100,
      sequence: 0,
    }));

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() =>
        expect(instancesFor(SESSION_A)[0]?.scrollToBottomCalls).toBe(1)
      );
      const terminal = instancesFor(SESSION_A)[0];
      const fittedResize = fake.callsFor("pty_resize").find(
        (call) =>
          String(call.args.sessionId) === SESSION_A &&
          Number(call.args.cols) === 100 &&
          Number(call.args.rows) === 30
      );
      const seedRequest = fake
        .callsFor("activate_terminal_output")
        .find((call) => String(call.args.sessionId) === SESSION_A);
      if (!fittedResize || !seedRequest) {
        throw new Error("expected the fitted resize and seed request");
      }

      expect(fake.calls.indexOf(fittedResize)).toBeLessThan(fake.calls.indexOf(seedRequest));
      expect({ cols: terminal.cols, rows: terminal.rows }).toEqual({ cols: 100, rows: 30 });
      expect(terminal.resizes).toEqual([{ cols: 100, rows: 30 }]);

      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .find((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled).toContain("cols=100");
      expect(settled).toContain("rows=30");
      expect(settled).toContain("snapshotCols=100");
      expect(settled).toContain("snapshotRows=30");
    } finally {
      rendered.cleanup();
    }
  });

  it("initial attach ends with the same geometry as detach -> reattach", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    fitViewport.cols = 100;
    fitViewport.rows = 30;
    fake.onInvoke("activate_terminal_output", (args) => ({
      sessionId: String(args.sessionId),
      data: String(args.sessionId) === SESSION_A ? MULTIVIEW : SNAP,
      rows: 30,
      cols: 100,
      sequence: 0,
    }));

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toEqual([MULTIVIEW]));
      const terminal = instancesFor(SESSION_A)[0];
      simulateParsedHistory(terminal, 120);
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      const initialGeometry = {
        cols: terminal.cols,
        rows: terminal.rows,
        viewportY: terminal.buffer.active.viewportY,
        baseY: terminal.buffer.active.baseY,
        bufferLength: terminal.buffer.active.length,
      };

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B])
      );
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      await waitFor(() => expect(terminal.writes).toEqual([MULTIVIEW, MULTIVIEW]));

      simulateParsedHistory(terminal, 120);
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(2));

      expect({
        cols: terminal.cols,
        rows: terminal.rows,
        viewportY: terminal.buffer.active.viewportY,
        baseY: terminal.buffer.active.baseY,
        bufferLength: terminal.buffer.active.length,
      }).toEqual(initialGeometry);
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);
      expect(terminal.screen).toEqual([MULTIVIEW]);

      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled).toHaveLength(2);
      for (const line of settled) {
        expect(line).toContain("cols=100");
        expect(line).toContain("rows=30");
        expect(line).toContain("snapshotCols=100");
        expect(line).toContain("snapshotRows=30");
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("snapshot replay never resizes a populated buffer on the aligned path", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    fitViewport.cols = 100;
    fitViewport.rows = 30;
    const attachGate = deferred<PtyScreenSnapshot | null>();
    fake.onInvoke("activate_terminal_output", () => attachGate.promise);

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      expect(terminal.resizes).toEqual([{ cols: 100, rows: 30 }]);
      expect(terminal.writes).toEqual([]);

      // Let the original second priming frame dedup while the snapshot is
      // still gated. Once the snapshot clears the key, the settle send is the
      // observable `sent` outcome pinned below.
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

      attachGate.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: 30,
        cols: 100,
        sequence: 0,
      });
      await waitFor(() => expect(terminal.writes).toEqual([SNAP]));
      expect(terminal.resizes).toEqual([{ cols: 100, rows: 30 }]);

      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .find((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled).toContain("cols=100");
      expect(settled).toContain("rows=30");
      expect(settled).toContain("snapshotCols=100");
      expect(settled).toContain("snapshotRows=30");
      expect(settled).toContain("resize=sent");
      expect(terminal.resizes).toEqual([{ cols: 100, rows: 30 }]);
    } finally {
      rendered.cleanup();
    }
  });

  it("initial attach completes without an animation frame while the document is hidden, and settles on restore", async () => {
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(document, "hidden");
    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    let cleanupRendered: (() => void) | null = null;

    try {
      const fake = new FakeTransport();
      setupTransport(fake);
      terminalStore.setActiveSessionForTests(SESSION_A);
      const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
      cleanupRendered = rendered.cleanup;

      await flushPromises();
      await flushPromises();
      expect(attachedSessionIds(fake)).toEqual([SESSION_A]);

      const currentGridResize = fake
        .callsFor("pty_resize")
        .find((call) => String(call.args.sessionId) === SESSION_A);
      const seedRequest = fake
        .callsFor("activate_terminal_output")
        .find((call) => String(call.args.sessionId) === SESSION_A);
      if (!currentGridResize || !seedRequest) {
        throw new Error("expected the hidden-path resize and seed request");
      }
      expect(fake.calls.indexOf(currentGridResize)).toBeLessThan(
        fake.calls.indexOf(seedRequest)
      );

      Object.defineProperty(document, "hidden", { configurable: true, value: false });
      const terminal = instancesFor(SESSION_A)[0];
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);
      expect(
        debug.mock.calls
          .map((call: unknown[]) => String(call[0]))
          .filter((line: string) =>
            line.startsWith("[terminal] attach " + SESSION_A + " settled:")
          )
      ).toHaveLength(1);
    } finally {
      cleanupRendered?.();
      if (hiddenDescriptor) {
        Object.defineProperty(document, "hidden", hiddenDescriptor);
      } else {
        Reflect.deleteProperty(document, "hidden");
      }
    }
  });

  // #1489 - every async continuation of an attach transaction must be inert
  // once a newer attach owns the session. Four scenarios: a held
  // generation-1 snapshot callback fenced by the shared drain (main, the
  // snapshot-class variant), a generation-1 live byte still queued when
  // generation 2 resets (sub-step A: the shared drain fences the FIFO
  // residue), a synchronous 50-MiB queue-guard throw (sub-step B: the drain
  // must release, not strand), and a generation-1 replay residue (sub-step C:
  // the fenced-replay registration makes generation 2's fence await it).
  it("a stale attach generation cannot mutate a newer replay", async () => {
    // ── main: generation 1's held snapshot callback stays inert ──
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("activate_terminal_output", (args) => ({
      sessionId: String(args.sessionId),
      data: SNAP,
      rows: null,
      cols: null,
      sequence: 0,
    }));
    const resizesOfA = () =>
      fake
        .callsFor("pty_resize")
        .filter((call) => String(call.args.sessionId) === SESSION_A)
        .length;
    const settledRecordsOf = (sessionId: string) =>
      debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter((line: string) =>
          line.startsWith("[terminal] attach " + sessionId + " settled:")
        ).length;

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];

      // Generation 1: the snapshot write is queued and held (gated).
      expect(terminal.writes).toEqual([SNAP]);

      // Switch to B: auto-complete, settles once.
      xterm.autoCompleteWrites = true;
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() =>
        expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1)
      );

      // Switch back to A: generation 2's snapshot write is NOT queued while
      // generation 1's gated snapshot callback is still held - the shared
      // drain fences the older generation's still-queued snapshot bytes (the
      // snapshot-class fence variant, proven before the release).
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      await flushPromises();
      expect(terminal.writes).toEqual([SNAP]);
      expect(terminal.scrollToBottomCalls).toBe(0);

      // Let the re-attach's priming sync frames run before counting resizes.
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

      const writesBefore = terminal.writes.length;
      const bottomsBefore = terminal.scrollToBottomCalls;
      const resizesBefore = resizesOfA();
      const recordsBefore = settledRecordsOf(SESSION_A) + settledRecordsOf(SESSION_B);

      // Complete generation 1's held callback: the drain releases, generation
      // 2's snapshot write queues and settles once, and generation 1's later
      // continuation (its flush) is inert - no additional writes, no
      // additional bottoming and no settled record for generation 1. The one
      // added pty_resize is generation 2's required real final confirmation.
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.writes).toEqual([SNAP, SNAP]));
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      expect(terminal.writes).toHaveLength(writesBefore + 1);
      expect(terminal.scrollToBottomCalls).toBe(bottomsBefore + 1);
      expect(resizesOfA()).toBe(resizesBefore + 1);
      expect(settledRecordsOf(SESSION_A) + settledRecordsOf(SESSION_B)).toBe(
        recordsBefore + 1
      );
      expect(queryTerminalTarget(terminal.element!).layoutPulse).toMatchObject({
        attachGeneration: 2,
      });
    } finally {
      rendered.cleanup();
    }

    // ── sub-step A: FIFO residue fenced by the shared drain ──
    xterm.autoCompleteWrites = false;
    const fakeA = new FakeTransport();
    setupTransport(fakeA);
    const attachGateA = deferred<PtyScreenSnapshot | null>();
    let attachesOfA = 0;
    fakeA.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        attachesOfA += 1;
        if (attachesOfA === 1) {
          return attachGateA.promise;
        }
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const renderedA = renderWithFakeTransport(() => <TerminalView />, fakeA);
    try {
      await waitFor(() => expect(attachedSessionIds(fakeA)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Generation-1 live byte: queued, not applied, while the fetch is held.
      fakeA.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 5,
      });
      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.screen).toEqual([]);

      attachGateA.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 0,
      });
      await flushPromises();

      // Switch to B and back to A; generation 2 has no live events of its own.
      xterm.autoCompleteWrites = true;
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() =>
        expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1)
      );
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fakeA)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      await flushPromises();

      // Generation 2's snapshot write is NOT queued until generation 1's
      // gated live write is completed: the shared drain fences the residue.
      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.screen).toEqual([]);

      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.writes).toEqual([LIVE, SNAP]));
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      // Final screen: no generation-1 byte before the generation-2 snapshot;
      // exactly-once content; one settle per generation.
      expect(terminal.screen).toEqual([SNAP]);
      expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1);
    } finally {
      renderedA.cleanup();
    }

    // ── sub-step B: synchronous 50-MiB queue-guard throw releases the drain ──
    xterm.autoCompleteWrites = false;
    const fakeB = new FakeTransport();
    setupTransport(fakeB);
    const attachGateB = deferred<PtyScreenSnapshot | null>();
    fakeB.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        return attachGateB.promise;
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const renderedB = renderWithFakeTransport(() => <TerminalView />, fakeB);
    try {
      await waitFor(() => expect(attachedSessionIds(fakeB)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // The queue guard throws synchronously before queueing; the transport
      // calls listeners synchronously, so the throw propagates out.
      terminal.writeThrows = true;
      expect(() =>
        fakeB.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 })
      ).toThrow();
      terminal.writeThrows = false;

      attachGateB.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 0,
      });
      await waitFor(() => expect(terminal.writes).toEqual([SNAP]));

      // The drain released (the snapshot write was queued); the retained byte
      // replays exactly once and the settle completes: inFlight never strands.
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.writes).toEqual([SNAP, LIVE]));
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.screen).toEqual([SNAP, LIVE]);
    } finally {
      renderedB.cleanup();
    }

    // ── sub-step C: unregistered replay residue fenced by the shared drain ──
    xterm.autoCompleteWrites = false;
    const fakeC = new FakeTransport();
    setupTransport(fakeC);
    const attachGateC = deferred<PtyScreenSnapshot | null>();
    let attachesOfC = 0;
    fakeC.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        attachesOfC += 1;
        if (attachesOfC === 1) {
          return attachGateC.promise;
        }
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const renderedC = renderWithFakeTransport(() => <TerminalView />, fakeC);
    try {
      await waitFor(() => expect(attachedSessionIds(fakeC)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];
      const settledBeforeC = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter(
          (line: string) =>
            line.startsWith("[terminal] attach ") && line.includes(" settled:")
        ).length;

      // Generation 1: a live byte queued (not applied) while the fetch is
      // held; the seed resolves and the flush queues the retained replay
      // bytes - all gated.
      fakeC.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 5,
      });
      attachGateC.resolve({
        sessionId: SESSION_A,
        data: SNAP,
        rows: null,
        cols: null,
        sequence: 0,
      });
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE]);

      completeWriteCallbacks(terminal);
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE, SNAP]);

      completeWriteCallbacks(terminal);
      await flushPromises();
      // Generation 1's replay byte is queued but not complete.
      expect(terminal.writes).toEqual([LIVE, SNAP, LIVE]);

      // Switch to B and back to A before generation 1's replay bytes
      // complete; generation 2's snapshot write is NOT queued until they do
      // (the fenced-replay registration makes the next generation's drain
      // await them).
      xterm.autoCompleteWrites = true;
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() =>
        expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1)
      );
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fakeC)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE, SNAP, LIVE]);
      expect(terminal.scrollToBottomCalls).toBe(0);

      // Complete generation 1's replay bytes: the drain releases, generation 2
      // re-seeds and settles once; generation 1's flush finalize is inert.
      completeWriteCallbacks(terminal);
      await waitFor(() =>
        expect(terminal.writes).toEqual([LIVE, SNAP, LIVE, SNAP])
      );
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      // No generation-1 byte before the generation-2 snapshot; exactly-once
      // screen content; exactly one settle per generation in this sub-step.
      expect(terminal.screen).toEqual([SNAP]);
      expect(instancesFor(SESSION_B)[0]?.screen).toEqual([SNAP]);
      const settledCount = () =>
        debug.mock.calls
          .map((call: unknown[]) => String(call[0]))
          .filter(
            (line: string) =>
              line.startsWith("[terminal] attach ") && line.includes(" settled:")
          ).length;
      expect(settledCount()).toBe(settledBeforeC + 2);
    } finally {
      renderedC.cleanup();
    }
  });

  // #1489 - ordinary live output must never bottom a viewport the user
  // deliberately scrolled up. The settle sequence is the only bottoming path;
  // `writeLivePtyOutput` never calls `scrollToBottom`. The settle-window
  // sub-step proves the wheel guard (4.5): a real wheel event in the
  // post-replay/pre-settle window suppresses the one-shot bottoming while the
  // settled record is still emitted.
  it("ordinary live output never bottoms a user who scrolled up", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const settledRecordsOf = (sessionId: string) =>
      debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter((line: string) =>
          line.startsWith("[terminal] attach " + sessionId + " settled:")
        ).length;

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];

      // Settled attach: the one-shot bottom ran exactly once.
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      simulateUserScrollUp(terminal);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 });
      await waitFor(() => expect(terminal.writes).toEqual([SNAP, LIVE]));

      expect(terminal.scrollToBottomCalls).toBe(1);
      expect(terminal.buffer.active.viewportY).toBe(0);

      // Settle-window sub-step (wheel guard, #1489 4.5): switch away and back
      // (second generation, gated writes); complete the second replay; before
      // the settle's rAF (a setTimeout(0) in this harness) fires, dispatch a
      // real wheel event on `terminal.element` - the marker suppresses only
      // the bottoming, not the settled record.
      xterm.autoCompleteWrites = false;
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)[0]?.writes).toEqual([SNAP]));
      completeWriteCallbacks(instancesFor(SESSION_B)[0]);
      await waitFor(() =>
        expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1)
      );

      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      // Generation 2's snapshot write is queued and gated; the second replay
      // runs when it completes.
      expect(terminal.writes).toEqual([SNAP, LIVE, SNAP]);

      // Complete the second replay. The settle's rAF is a setTimeout(0) and
      // has not fired yet when the completion returns, so the wheel event
      // lands inside the settle window.
      completeWriteCallbacks(terminal);
      terminal.element?.dispatchEvent(new WheelEvent("wheel"));

      await waitFor(() => expect(settledRecordsOf(SESSION_A)).toBe(2));
      expect(terminal.scrollToBottomCalls).toBe(1);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 - an attach settle over a meaningful multi-viewport history must
  // bottom to the current screen: bufferLength and baseY reflect the parsed
  // history, and the settled record carries the normal-buffer evidence.
  it("attach settle preserves meaningful multi-viewport history", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId !== SESSION_A) {
        return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
      }
      return {
        sessionId,
        data: MULTIVIEW,
        rows: 24,
        cols: 80,
        sequence: 0,
      };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(instancesFor(SESSION_A)[0]?.writes).toHaveLength(1));
      const terminal = instancesFor(SESSION_A)[0];

      // The snapshot write is queued but not applied; the reset has already
      // zeroed the buffer metrics, so the parsed history can be simulated
      // before the write completes and the settle runs.
      simulateParsedHistory(terminal, 120);
      completeWriteCallbacks(terminal);

      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.buffer.active.viewportY).toBe(96);
      expect(terminal.buffer.active.baseY).toBe(96);
      expect(terminal.buffer.active.length).toBe(120);

      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .find((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled).toContain("type=normal");
      expect(settled).toContain("snapshotCols=80");
      expect(settled).toContain("snapshotRows=24");
      expect(settled).toContain("seedBytes=" + String(MULTIVIEW.length));

      // Scrolling to the top shows the reconstructed history, not a synthetic
      // empty region.
      simulateUserScrollUp(terminal);
      expect(terminal.screen).not.toEqual([]);
      expect(terminal.screen[0]).toEqual(MULTIVIEW);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 - a near-64-KiB ring replay stays exactly-once with one settle:
  // the live bytes are written on arrival, the snapshot re-seeds, and the
  // retention replays after it — never duplicated.
  it("near-64-KiB ring replay stays exactly-once with one settle", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const attachGate = deferred<PtyScreenSnapshot | null>();
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        return attachGate.promise;
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Overlapping live events while the seed is in flight.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 });
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 6 });
      attachGate.resolve({
        sessionId: SESSION_A,
        data: RING_64K,
        rows: null,
        cols: null,
        sequence: 0,
      });

      // Ring bytes, then the overlapping live bytes, exactly once.
      await waitFor(() => expect(terminal.screen).toEqual([RING_64K, LIVE, LIVE]));
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);
      expect(terminal.writes).toEqual([LIVE, LIVE, RING_64K, LIVE, LIVE]);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 Codex lifecycle: an idle multi-viewport scrollback with overlapping
  // live events settles at the bottom; a user scroll-up survives ordinary live
  // output and the re-attach; the second attach's settle records the
  // alternate-screen evidence (the alternate buffer has no scrollback, so the
  // bottoming is a no-op there).
  it("Codex lifecycle: idle multi-viewport replay, overlap, scroll-up, and re-attach", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const attachGates = [
      deferred<PtyScreenSnapshot | null>(),
      deferred<PtyScreenSnapshot | null>(),
    ];
    let attachesOfA = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        const gate = attachGates[attachesOfA];
        attachesOfA += 1;
        return gate.promise;
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Idle multi-viewport replay with overlapping live events.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 });
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: GONE, sequence: 6 });
      attachGates[0].resolve({
        sessionId: SESSION_A,
        data: MULTIVIEW,
        rows: 24,
        cols: 80,
        sequence: 0,
      });

      await waitFor(() => expect(terminal.screen).toEqual([MULTIVIEW, LIVE, GONE]));
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);

      // Scroll up: further live output stays put. (`writes` also records the
      // live arrivals: LIVE, GONE, then the snapshot, the replay, and LIVE.)
      simulateUserScrollUp(terminal);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 7 });
      await waitFor(() =>
        expect(terminal.writes).toEqual([LIVE, GONE, MULTIVIEW, LIVE, GONE, LIVE])
      );
      expect(terminal.scrollToBottomCalls).toBe(1);
      expect(terminal.buffer.active.viewportY).toBe(0);

      // Switch away and back: exactly one settle per attach.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(detachedSessionIds(fake)).toEqual([SESSION_A]));
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B])
      );

      // Alternate-screen evidence: before the second attach's settle.
      terminal.buffer.active.type = "alternate";
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      attachGates[1].resolve({
        sessionId: SESSION_A,
        data: MULTIVIEW,
        rows: 24,
        cols: 80,
        sequence: 0,
      });
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(2));

      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter((line: string) =>
          line.startsWith("[terminal] attach " + SESSION_A + " settled:")
        );
      expect(settled[0]).toContain("type=normal");
      expect(settled[1]).toContain("type=alternate");
      expect(terminal.buffer.active.viewportY).toBe(terminal.buffer.active.baseY);

      // No stale mutation: the re-seed wiped the pre-switch content and the
      // second seed stands exactly once.
      expect(terminal.screen).toEqual([MULTIVIEW]);
    } finally {
      rendered.cleanup();
    }
  });

  // #1489 Pi lifecycle: a near-64-KiB ring replay with a switch away
  // mid-replay and back. Both generations' pipelines are held on gated
  // writes; completing them in reverse order (B's generation, then A's)
  // proves exactly-once screen content, one settle per attach, and inert
  // stale callbacks.
  it("Pi lifecycle: ring replay with switch-away/back and exactly-once live bytes", async () => {
    xterm.autoCompleteWrites = false;
    const fake = new FakeTransport();
    setupTransport(fake);
    const attachGate = deferred<PtyScreenSnapshot | null>();
    let attachesOfA = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      const sessionId = String(args.sessionId);
      if (sessionId === SESSION_A) {
        attachesOfA += 1;
        if (attachesOfA === 1) {
          return attachGate.promise;
        }
        return { sessionId, data: RING_64K, rows: null, cols: null, sequence: 0 };
      }
      return { sessionId, data: SNAP, rows: null, cols: null, sequence: 0 };
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(attachedSessionIds(fake)).toEqual([SESSION_A]));
      const terminal = instancesFor(SESSION_A)[0];

      // Generation 1: a live byte queued (not applied) while the fetch is
      // held; the ring snapshot arrives after it.
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 5 });
      expect(terminal.writes).toEqual([LIVE]);
      attachGate.resolve({
        sessionId: SESSION_A,
        data: RING_64K,
        rows: null,
        cols: null,
        sequence: 0,
      });
      await flushPromises();

      // Switch away mid-replay: B attaches and its snapshot write is queued.
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instancesFor(SESSION_B)[0]?.writes).toEqual([SNAP]));

      // Complete B's held generation while B is still current: it settles
      // exactly once (one settle per attach), so the later stale A callbacks
      // have nothing left to mutate.
      completeWriteCallbacks(instancesFor(SESSION_B)[0]);
      await waitFor(() =>
        expect(instancesFor(SESSION_B)[0]?.scrollToBottomCalls).toBe(1)
      );
      expect(instancesFor(SESSION_B)[0]?.screen).toEqual([SNAP]);

      // Switch back: generation 2's ring write must NOT be queued until
      // generation 1's gated live byte has parsed (the shared drain fences
      // the FIFO residue).
      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() =>
        expect(attachedSessionIds(fake)).toEqual([SESSION_A, SESSION_B, SESSION_A])
      );
      await flushPromises();
      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.screen).toEqual([]);

      // Complete A's FIFO: generation 1's live byte releases the drain —
      // generation 1's continuation aborts as stale (inert) and generation 2
      // proceeds: reset, then its own ring write, then its settle.
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.writes).toEqual([LIVE, RING_64K]));
      completeWriteCallbacks(terminal);
      await waitFor(() => expect(terminal.scrollToBottomCalls).toBe(1));

      // Exactly-once screen content, one settle per attach, no stale byte
      // inside the newer replay.
      expect(terminal.screen).toEqual([RING_64K]);
      expect(instancesFor(SESSION_B)[0]?.screen).toEqual([SNAP]);
      const settled = debug.mock.calls
        .map((call: unknown[]) => String(call[0]))
        .filter(
          (line: string) =>
            line.startsWith("[terminal] attach ") && line.includes(" settled:")
        );
      expect(settled).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });
});
