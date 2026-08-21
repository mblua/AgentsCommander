// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import type { JSX } from "solid-js";
import TerminalView from "./TerminalView";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { terminalStore } from "../stores/terminal";
import { SESSION_A, SESSION_B } from "../../shared/testing/session-selection";
import {
  TEST_TERMINAL_DOCUMENT_EPOCH,
  terminalActivationWire,
  terminalSeedlessActivationWire,
} from "../../shared/testing/terminal-output";

interface DeterministicAnimationFrames {
  readonly flushFrame: () => Promise<boolean>;
  readonly restore: () => void;
}

function installTerminalDomStubs(): () => void {
  const previousResizeObserver = globalThis.ResizeObserver;

  class NoopResizeObserver implements ResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }

  Object.defineProperty(globalThis, "ResizeObserver", {
    configurable: true,
    writable: true,
    value: NoopResizeObserver,
  });
  return () => {
    Object.defineProperty(globalThis, "ResizeObserver", {
      configurable: true,
      writable: true,
      value: previousResizeObserver,
    });
  };
}

function installDeterministicAnimationFrames(): DeterministicAnimationFrames {
  const previousRequestAnimationFrame = globalThis.requestAnimationFrame;
  const previousCancelAnimationFrame = globalThis.cancelAnimationFrame;
  let nextHandle = 1;
  let timestamp = 0;
  let queued: Array<{ handle: number; callback: FrameRequestCallback }> = [];

  Object.defineProperty(globalThis, "requestAnimationFrame", {
    configurable: true,
    writable: true,
    value: (callback: FrameRequestCallback): number => {
      const handle = nextHandle;
      nextHandle += 1;
      queued.push({ handle, callback });
      return handle;
    },
  });
  Object.defineProperty(globalThis, "cancelAnimationFrame", {
    configurable: true,
    writable: true,
    value: (handle: number): void => {
      queued = queued.filter((frame) => frame.handle !== handle);
    },
  });

  return {
    flushFrame: async (): Promise<boolean> => {
      const ready = queued;
      queued = [];
      for (const frame of ready) {
        frame.callback(timestamp);
      }
      timestamp += 16;
      await Promise.resolve();
      return ready.length > 0;
    },
    restore: (): void => {
      queued = [];
      Object.defineProperty(globalThis, "requestAnimationFrame", {
        configurable: true,
        writable: true,
        value: previousRequestAnimationFrame,
      });
      Object.defineProperty(globalThis, "cancelAnimationFrame", {
        configurable: true,
        writable: true,
        value: previousCancelAnimationFrame,
      });
    },
  };
}

function renderWithFakeTransport(
  component: () => JSX.Element,
  fake: FakeTransport,
): { readonly root: HTMLDivElement; readonly cleanup: () => Promise<void> } {
  const restoreTransport = __setTransportForTests(fake);
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(component, root);
  return {
    root,
    cleanup: async () => {
      dispose();
      await flushPromises();
      restoreTransport();
      root.remove();
    },
  };
}

async function waitFor(assertion: () => void, timeoutMs = 1_000): Promise<void> {
  const startedAt = Date.now();
  let lastError: unknown;
  while (Date.now() - startedAt < timeoutMs) {
    try {
      assertion();
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
  if (lastError instanceof Error) throw lastError;
  throw new Error(`Timed out after ${timeoutMs}ms`);
}

interface FakeTerminalBuffer {
  type: "normal" | "alternate";
  viewportY: number;
  baseY: number;
  length: number;
  getLine: (index: number) =>
    | { getCell: (col: number) => { getChars: () => string } | undefined }
    | undefined;
}

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: number[][];
  screen: number[][];
  resets: number;
  disposed: boolean;
  bottomCalls: number;
  activeBuffer: "normal" | "alternate";
  viewportY: number;
  baseY: number;
  bufferLength: number;
  missingLine: number | null;
  missingCell: number | null;
  width: number;
  height: number;
  screenElement: HTMLDivElement | null;
  canvasElement: HTMLCanvasElement | null;
  readonly buffer: { active: FakeTerminalBuffer };
  releaseNextWriteCallback: () => void;
  pendingWriteCallbacks: () => number;
  resize: (cols: number, rows: number) => void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
  events: [] as string[],
}));

const fit = vi.hoisted(() => ({
  proposed: { cols: 80, rows: 24 },
}));

interface FakeWebglInstance {
  disposed: boolean;
  lose: () => void;
}

const webgl = vi.hoisted(() => ({
  instances: [] as FakeWebglInstance[],
  failConstruction: false,
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    writes: number[][] = [];
    screen: number[][] = [];
    resets = 0;
    disposed = false;
    bottomCalls = 0;
    activeBuffer: "normal" | "alternate" = "normal";
    viewportY = 0;
    baseY = 0;
    bufferLength = 24;
    missingLine: number | null = null;
    missingCell: number | null = null;
    width = 800;
    height = 480;
    screenElement: HTMLDivElement | null = null;
    canvasElement: HTMLCanvasElement | null = null;
    private hasText = false;
    private bottomHasText = false;
    private semanticHasTextOverride: boolean | null = null;
    private semanticOverrideToken = 0;
    private readonly writeCallbacks: Array<{ callback: () => void; bytes: number[] }> = [];
    private readonly resizeCallbacks = new Set<(size: { cols: number; rows: number }) => void>();

    constructor() {
      xterm.instances.push(this);
    }

    get buffer(): { active: FakeTerminalBuffer } {
      return {
        active: {
          type: this.activeBuffer,
          viewportY: this.viewportY,
          baseY: this.baseY,
          length: this.bufferLength,
          getLine: (index: number) => {
            const row = index - this.baseY;
            if (row < 0 || row >= this.rows || this.missingLine === row) {
              return undefined;
            }
            return {
              getCell: (col: number) => {
                if (col < 0 || col >= this.cols || this.missingCell === col) {
                  return undefined;
                }
                const activeHasText = this.semanticHasTextOverride ?? this.hasText;
                const lineHasText =
                  row === 0 ? activeHasText : row === this.rows - 1 && this.bottomHasText;
                return { getChars: () => (lineHasText && col === 0 ? "x" : "") };
              },
            };
          },
        },
      };
    }

    loadAddon(addon: { activate?: (terminal: FakeTerminalInstance) => void }): void {
      addon.activate?.(this);
    }

    open(container: HTMLElement): void {
      const element = document.createElement("div");
      element.className = "xterm";
      const screen = document.createElement("div");
      screen.className = "xterm-screen";
      const canvas = document.createElement("canvas");
      canvas.width = this.width;
      canvas.height = this.height;
      screen.appendChild(canvas);
      element.appendChild(screen);
      container.appendChild(element);
      const rect = () => new DOMRect(0, 0, this.width, this.height);
      container.getBoundingClientRect = rect;
      element.getBoundingClientRect = rect;
      screen.getBoundingClientRect = rect;
      this.element = element;
      this.screenElement = screen;
      this.canvasElement = canvas;
    }

    focus(): void {}
    dispose(): void {
      this.disposed = true;
      this.writeCallbacks.length = 0;
      this.resizeCallbacks.clear();
    }
    write(data: Uint8Array, callback?: () => void): void {
      const bytes = Array.from(data);
      this.writes.push(bytes);
      xterm.events.push(`write:${bytes.length}`);
      if (bytes.length > 0) {
        this.screen.push(bytes);
        this.hasText = true;
      }
      if (callback) this.writeCallbacks.push({ callback, bytes });
    }
    releaseNextWriteCallback(): void {
      const pending = this.writeCallbacks.shift();
      if (!pending) throw new Error("No pending xterm write callback");
      xterm.events.push("writeCallback");
      this.semanticHasTextOverride = pending.bytes.length > 0;
      this.semanticOverrideToken += 1;
      const token = this.semanticOverrideToken;
      pending.callback();
      queueMicrotask(() => {
        if (this.semanticOverrideToken === token) this.semanticHasTextOverride = null;
      });
    }
    pendingWriteCallbacks(): number {
      return this.writeCallbacks.length;
    }
    reset(): void {
      this.resets += 1;
      this.screen.length = 0;
      this.hasText = false;
      this.bottomHasText = false;
      xterm.events.push("reset");
    }
    scrollToBottom(): void {
      this.bottomCalls += 1;
      this.viewportY = this.baseY;
      xterm.events.push("bottom");
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
    onResize(callback: (size: { cols: number; rows: number }) => void): { dispose: () => void } {
      this.resizeCallbacks.add(callback);
      return { dispose: () => this.resizeCallbacks.delete(callback) };
    }
    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
      this.bufferLength = Math.max(this.bufferLength, this.baseY + rows);
      xterm.events.push(`xtermResize:${cols}x${rows}`);
      for (const callback of this.resizeCallbacks) callback({ cols, rows });
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: FakeTerminalInstance | null = null;
    activate(terminal: FakeTerminalInstance): void {
      this.terminal = terminal;
    }
    fit(): void {
      xterm.events.push("fit");
      this.terminal?.resize(fit.proposed.cols, fit.proposed.rows);
    }
    proposeDimensions(): { cols: number; rows: number } {
      return { ...fit.proposed };
    }
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class implements FakeWebglInstance {
    disposed = false;
    private readonly callbacks = new Set<() => void>();
    constructor() {
      if (webgl.failConstruction) throw new Error("webgl unavailable");
      webgl.instances.push(this);
    }
    activate(): void {}
    onContextLoss(callback: () => void): { dispose: () => void } {
      this.callbacks.add(callback);
      return { dispose: () => this.callbacks.delete(callback) };
    }
    lose(): void {
      for (const callback of [...this.callbacks]) callback();
    }
    dispose(): void {
      this.disposed = true;
    }
  },
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("../../shared/platform", () => ({ isTauri: true, isBrowser: false }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {} }),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "terminal" }),
}));

const SNAP = [83, 78, 65, 80];
const LIVE = [76, 73, 86, 69];

interface Deferred<T> {
  readonly promise: Promise<T>;
  readonly resolve: (value: T) => void;
  readonly reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((next, fail) => {
    resolve = next;
    reject = fail;
  });
  return { promise, resolve, reject };
}

async function flushPromises(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.resolve("terminal_output_document_epoch", TEST_TERMINAL_DOCUMENT_EPOCH);
  fake.resolve("detach_terminal_output", undefined);
  fake.resolve("cancel_terminal_output_activation", undefined);
  fake.resolve("record_terminal_attach_observation", undefined);
  fake.onInvoke("activate_terminal_output", (args) =>
    terminalActivationWire(args, { replayData: SNAP }),
  );
}

function instanceFor(sessionId: string): FakeTerminalInstance {
  const instance = xterm.instances.find(
    (candidate) =>
      !candidate.disposed &&
      candidate.element?.parentElement?.getAttribute("data-ac-session-id") === sessionId,
  );
  if (!instance) throw new Error(`No xterm instance for ${sessionId}`);
  return instance;
}

function observationStages(fake: FakeTransport): string[] {
  return fake.callsFor("record_terminal_attach_observation").map((call) => {
    const observation = call.args.observation;
    if (typeof observation !== "object" || observation === null || !("stage" in observation)) {
      throw new Error("Malformed test observation");
    }
    return String(observation.stage);
  });
}

async function releaseReplayAndFence(terminal: FakeTerminalInstance): Promise<void> {
  await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
  terminal.releaseNextWriteCallback();
  await flushPromises();
  await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
  terminal.releaseNextWriteCallback();
  await flushPromises();
}

async function finishFrames(frames: DeterministicAnimationFrames): Promise<void> {
  expect(await frames.flushFrame()).toBe(true);
  await flushPromises();
  expect(await frames.flushFrame()).toBe(true);
  await flushPromises();
}

describe("TerminalView deterministic attachment transaction (#1478)", () => {
  let cleanupDom: (() => void) | null = null;
  let frames: DeterministicAnimationFrames | null = null;
  let warn: ReturnType<typeof vi.spyOn>;
  let error: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    cleanupDom = installTerminalDomStubs();
    frames = installDeterministicAnimationFrames();
    terminalStore.resetForTests();
    xterm.instances.length = 0;
    xterm.events.length = 0;
    webgl.instances.length = 0;
    webgl.failConstruction = false;
    fit.proposed = { cols: 80, rows: 24 };
    warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    error = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    const unexpectedWarnings = warn.mock.calls.filter((call: unknown[]) => {
      const message = call[0];
      return typeof message !== "string" || !message.startsWith("[terminal-snapshot]");
    });
    expect(unexpectedWarnings).toEqual([]);
    expect(error).not.toHaveBeenCalled();
    terminalStore.resetForTests();
    frames?.restore();
    frames = null;
    cleanupDom?.();
    cleanupDom = null;
    warn.mockRestore();
    error.mockRestore();
  });

  it("orders replay, retained fence, first RAF, suppressed fit, confirmed resize, one bottom, and second RAF", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const resizeGate = deferred<void>();
    fake.onInvoke("pty_resize", () => resizeGate.promise);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("activate_terminal_output")).toHaveLength(1));
      const terminal = instanceFor(SESSION_A);
      await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
      expect(observationStages(fake)).toEqual([]);
      expect(terminal.bottomCalls).toBe(0);

      terminal.releaseNextWriteCallback();
      await flushPromises();
      expect(observationStages(fake)).toEqual([]);
      expect(terminal.pendingWriteCallbacks()).toBe(1);

      terminal.releaseNextWriteCallback();
      await waitFor(() => expect(observationStages(fake)).toEqual(["postWrite"]));
      expect(fake.callsFor("pty_resize")).toHaveLength(0);

      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() => expect(fake.callsFor("pty_resize")).toHaveLength(1));
      expect(terminal.bottomCalls).toBe(0);
      expect(observationStages(fake)).toEqual(["postWrite"]);

      resizeGate.resolve();
      await waitFor(() => expect(observationStages(fake)).toEqual(["postWrite", "postFit"]));
      expect(terminal.bottomCalls).toBe(1);

      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() =>
        expect(observationStages(fake)).toEqual(["postWrite", "postFit", "settled"]),
      );
      expect(terminal.bottomCalls).toBe(1);
      expect(xterm.events).toEqual([
        "reset",
        "write:4",
        "writeCallback",
        "write:0",
        "writeCallback",
        "fit",
        "xtermResize:80x24",
        "bottom",
      ]);
      const observations = fake
        .callsFor("record_terminal_attach_observation")
        .map((call) => call.args.observation);
      expect(observations).toHaveLength(3);
      expect(JSON.stringify(observations)).not.toMatch(
        /replayData|terminalBytes|prompt|command|workingDirectory|cwd|argv|environment|userText|error/i,
      );
    } finally {
      await rendered.cleanup();
    }
  });

  it("waits for postWrite observation acceptance before advancing to the first frame", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const observationGate = deferred<void>();
    fake.onInvoke("record_terminal_attach_observation", () => observationGate.promise);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      expect(observationStages(fake)).toEqual(["postWrite"]);
      expect(await frames!.flushFrame()).toBe(false);

      observationGate.resolve();
      await flushPromises();
      await finishFrames(frames!);
      expect(observationStages(fake)).toEqual(["postWrite", "postFit", "settled"]);
    } finally {
      await rendered.cleanup();
    }
  });

  it("reports current post-fit grid values separately from snapshot dimensions", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, {
        replayData: SNAP,
        cols: 81,
        rows: 27,
      }),
    );
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      const observations = fake.callsFor("record_terminal_attach_observation");
      expect(observations[0].args.observation).toMatchObject({
        stage: "postWrite",
        parserCols: 81,
        parserRows: 27,
        conptyCols: 81,
        conptyRows: 27,
        snapshotCols: 81,
        snapshotRows: 27,
        xtermCols: 81,
        xtermRows: 27,
        gridAgreement: true,
      });
      expect(observations[2].args.observation).toMatchObject({
        stage: "settled",
        parserCols: 80,
        parserRows: 24,
        conptyCols: 80,
        conptyRows: 24,
        snapshotCols: 81,
        snapshotRows: 27,
        xtermCols: 80,
        xtermRows: 24,
        visibleRowCount: 24,
        missingVisibleRowCount: 0,
        gridAgreement: true,
        resizeConfirmed: true,
        expectedActiveScreenHasText: true,
        observedActiveScreenHasText: true,
        expectedBottomLineHasText: false,
        observedBottomLineHasText: false,
      });
    } finally {
      await rendered.cleanup();
    }
  });

  it("writes live output immediately, then reset-replays only events above the snapshot watermark", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const activation = deferred<unknown>();
    let activationArgs: Record<string, unknown> | null = null;
    fake.onInvoke("activate_terminal_output", (args) => {
      activationArgs = args;
      return activation.promise;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(activationArgs).not.toBeNull());
      const terminal = instanceFor(SESSION_A);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE, sequence: 2 });
      expect(terminal.screen).toEqual([LIVE]);

      activation.resolve(
        terminalActivationWire(activationArgs!, { replayData: SNAP, sequence: 1 }),
      );
      await waitFor(() => expect(terminal.resets).toBe(1));
      expect(terminal.screen).toEqual([SNAP]);
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      expect(terminal.screen).toEqual([SNAP, LIVE]);
      expect(terminal.bottomCalls).toBe(1);
    } finally {
      await rendered.cleanup();
    }
  });

  it("keeps seedless live output exactly once and reports the typed fallback, never success", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const activation = deferred<unknown>();
    let activationArgs: Record<string, unknown> | null = null;
    fake.onInvoke("activate_terminal_output", (args) => {
      activationArgs = args;
      return activation.promise;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(activationArgs).not.toBeNull());
      const terminal = instanceFor(SESSION_A);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE });
      expect(terminal.screen).toEqual([LIVE]);
      activation.resolve(
        terminalSeedlessActivationWire(activationArgs!, "seedlessParserUnavailable"),
      );
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      expect(terminal.resets).toBe(0);
      expect(terminal.screen).toEqual([LIVE]);
      expect(observationStages(fake)).toEqual(["postWrite", "postFit", "aborted"]);
      const terminalObservation = fake.callsFor("record_terminal_attach_observation")[2];
      expect(terminalObservation.args.observation).toMatchObject({
        outcome: "seedlessParserUnavailable",
      });
    } finally {
      await rendered.cleanup();
    }
  });

  it("converts a malformed activation to snapshotDiscarded without reset or byte duplication", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("activate_terminal_output", (args) => ({
      snapshot: { replayData: [256] },
      attachGeneration: args.attachGeneration,
      documentEpoch: args.documentEpoch,
    }));
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      fake.emitFromBackend("pty_output", { sessionId: SESSION_A, data: LIVE });
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      expect(terminal.resets).toBe(0);
      expect(terminal.screen).toEqual([LIVE]);
      expect(observationStages(fake)).toEqual(["postWrite", "postFit", "aborted"]);
      expect(fake.callsFor("record_terminal_attach_observation")[2].args.observation).toMatchObject({
        outcome: "snapshotDiscarded",
      });
    } finally {
      await rendered.cleanup();
    }
  });

  it("supersedes a hung A activation, attaches B with a new generation, and compensates a late A resolve", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const activationA = deferred<unknown>();
    let argsA: Record<string, unknown> | null = null;
    fake.onInvoke("activate_terminal_output", (args) => {
      if (String(args.sessionId) === SESSION_A) {
        argsA = args;
        return activationA.promise;
      }
      return terminalActivationWire(args, { replayData: SNAP });
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(argsA).not.toBeNull());
      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() =>
        expect(fake.callsFor("activate_terminal_output").map((call) => call.args.sessionId)).toEqual([
          SESSION_A,
          SESSION_B,
        ]),
      );
      const terminalB = instanceFor(SESSION_B);
      await releaseReplayAndFence(terminalB);
      await finishFrames(frames!);
      expect(fake.callsFor("activate_terminal_output").map((call) => call.args.attachGeneration)).toEqual([
        1,
        2,
      ]);

      activationA.resolve(terminalActivationWire(argsA!, { replayData: SNAP }));
      await flushPromises();
      expect(
        fake
          .callsFor("cancel_terminal_output_activation")
          .some((call) => call.args.sessionId === SESSION_A && call.args.attachGeneration === 1),
      ).toBe(true);
      expect(instanceFor(SESSION_B).bottomCalls).toBe(1);
    } finally {
      await rendered.cleanup();
    }
  });

  it("retains the newest of more than eight late activations when exact cleanup keeps rejecting", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const activationGates = Array.from({ length: 10 }, () => deferred<unknown>());
    const activationArgs: Array<Record<string, unknown> | null> = Array.from(
      { length: activationGates.length },
      () => null,
    );
    let rejectLateCleanup = false;
    const rejectedCleanupGenerations: number[] = [];
    fake.onInvoke("activate_terminal_output", (args) => {
      const generation = Number(args.attachGeneration);
      const gate = activationGates[generation - 1];
      if (!gate) throw new Error(`Unexpected attachment generation ${generation}`);
      activationArgs[generation - 1] = args;
      return gate.promise;
    });
    fake.onInvoke("cancel_terminal_output_activation", (args) => {
      if (!rejectLateCleanup) return undefined;
      rejectedCleanupGenerations.push(Number(args.attachGeneration));
      throw new Error("test permanent late cleanup rejection");
    });

    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("activate_terminal_output")).toHaveLength(1));
      for (let generation = 2; generation <= activationGates.length; generation += 1) {
        terminalStore.setActiveSessionForTests(
          generation % 2 === 0 ? SESSION_B : SESSION_A,
        );
        await waitFor(() =>
          expect(fake.callsFor("activate_terminal_output")).toHaveLength(generation),
        );
      }

      terminalStore.setActiveSessionForTests(null);
      await flushPromises();
      rejectLateCleanup = true;
      for (const [index, gate] of activationGates.entries()) {
        const args = activationArgs[index];
        if (!args) throw new Error(`Missing activation args for generation ${index + 1}`);
        gate.resolve(terminalActivationWire(args, { replayData: SNAP }));
      }

      await waitFor(
        () =>
          expect(
            rejectedCleanupGenerations.filter(
              (generation) => generation === activationGates.length,
            ),
          ).toHaveLength(3),
        2_000,
      );
      const capacityDiagnostics = warn.mock.calls
        .map((call: unknown[]) => call[0])
        .filter(
          (message: unknown): message is string =>
            typeof message === "string" && message.includes("outcome=capacity_exceeded"),
        );
      expect(capacityDiagnostics).toEqual([]);
    } finally {
      rejectLateCleanup = false;
      await rendered.cleanup();
    }
  });

  it("aborts before postWrite when a replay-barrier visible line is undefined", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
      terminal.missingLine = 0;
      terminal.releaseNextWriteCallback();
      await waitFor(() => expect(observationStages(fake)).toEqual(["aborted"]));
      expect(fake.callsFor("record_terminal_attach_observation")[0].args.observation).toMatchObject({
        outcome: "invariantFailed",
        visibleRowsPresent: false,
      });
      expect(terminal.bottomCalls).toBe(0);
    } finally {
      await rendered.cleanup();
    }
  });

  it("uses DOM/lost renderer truth after WebGL context loss and still settles", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      webgl.instances[0].lose();
      terminal.canvasElement?.remove();
      terminal.canvasElement = null;
      await finishFrames(frames!);
      const settled = fake.callsFor("record_terminal_attach_observation")[2].args.observation;
      expect(settled).toMatchObject({
        stage: "settled",
        renderer: "dom",
        contextState: "lost",
      });
      expect(settled).not.toHaveProperty("canvasWidth");
      expect(settled).not.toHaveProperty("canvasHeight");
      expect(webgl.instances[0].disposed).toBe(true);
    } finally {
      await rendered.cleanup();
    }
  });

  it("settles through the known DOM/unavailable renderer fallback", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    webgl.failConstruction = true;
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      terminal.canvasElement?.remove();
      terminal.canvasElement = null;
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      const settled = fake.callsFor("record_terminal_attach_observation")[2].args.observation;
      expect(settled).toMatchObject({
        stage: "settled",
        renderer: "dom",
        contextState: "unavailable",
      });
      expect(settled).not.toHaveProperty("canvasWidth");
      expect(settled).not.toHaveProperty("canvasHeight");
      expect(webgl.instances).toHaveLength(0);
    } finally {
      await rendered.cleanup();
    }
  });

  it("checks snapshot semantics at the replay callback before retained bytes change the screen", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const activation = deferred<unknown>();
    let activationArgs: Record<string, unknown> | null = null;
    fake.onInvoke("activate_terminal_output", (args) => {
      activationArgs = args;
      return activation.promise;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(activationArgs).not.toBeNull());
      const terminal = instanceFor(SESSION_A);
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 1,
      });
      activation.resolve(
        terminalActivationWire(activationArgs!, {
          replayData: [],
          sequence: 0,
          activeScreenHasText: false,
          activeBottomLineHasText: false,
        }),
      );
      await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
      terminal.releaseNextWriteCallback();
      await flushPromises();
      expect(observationStages(fake)).toEqual([]);
      expect(terminal.screen).toEqual([LIVE]);
      expect(terminal.pendingWriteCallbacks()).toBe(1);
      terminal.releaseNextWriteCallback();
      await flushPromises();
      expect(observationStages(fake)).toEqual(["postWrite"]);
      await finishFrames(frames!);
      expect(terminal.screen).toEqual([LIVE]);
      expect(observationStages(fake)).toEqual(["postWrite", "postFit", "settled"]);
    } finally {
      await rendered.cleanup();
    }
  });

  it.each([
    ["history-disabled", "screenOnlyHistoryDisabled"],
    ["checkpoint-unavailable", "screenOnlyCheckpointUnavailable"],
  ] as const)("settles layout for %s without reporting semantic success", async (_label, replayStage) => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, {
        replayData: SNAP,
        replayStage,
        ...(replayStage === "screenOnlyCheckpointUnavailable"
          ? {
              activeBuffer: "alternate" as const,
              alternateEntryMode: "mode47" as const,
              normalScreenIncluded: false,
            }
          : {}),
      }),
    );
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await waitFor(() => expect(terminal.pendingWriteCallbacks()).toBe(1));
      if (replayStage === "screenOnlyCheckpointUnavailable") {
        terminal.activeBuffer = "alternate";
      }
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      const finalObservation = fake.callsFor("record_terminal_attach_observation")[2].args.observation;
      expect(finalObservation).toMatchObject({ stage: "settled", outcome: replayStage });
      expect(finalObservation).not.toMatchObject({ outcome: "success" });
    } finally {
      await rendered.cleanup();
    }
  });

  it.each([
    ["zero geometry", (terminal: FakeTerminalInstance) => {
      terminal.width = 0;
      terminal.height = 0;
    }],
    ["disconnected screen", (terminal: FakeTerminalInstance) => terminal.screenElement?.remove()],
    ["insufficient buffer", (terminal: FakeTerminalInstance) => {
      terminal.bufferLength = terminal.rows - 1;
    }],
    ["missing cell", (terminal: FakeTerminalInstance) => {
      terminal.missingCell = 0;
    }],
    ["missing WebGL canvas", (terminal: FakeTerminalInstance) => {
      terminal.canvasElement?.remove();
      terminal.canvasElement = null;
    }],
    ["missing visible row", (terminal: FakeTerminalInstance) => {
      terminal.missingLine = 0;
    }],
  ] as const)("aborts settlement for %s after postFit", async (label, breakInvariant) => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() => expect(observationStages(fake)).toEqual(["postWrite", "postFit"]));
      breakInvariant(terminal);
      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() =>
        expect(observationStages(fake)).toEqual(["postWrite", "postFit", "aborted"]),
      );
      expect(fake.callsFor("record_terminal_attach_observation")[2].args.observation).toMatchObject({
        outcome: "invariantFailed",
      });
      if (label === "missing visible row") {
        expect(
          fake.callsFor("record_terminal_attach_observation")[2].args.observation,
        ).toMatchObject({ visibleRowCount: 23, missingVisibleRowCount: 1 });
      }
    } finally {
      await rendered.cleanup();
    }
  });

  it("records mocked 160 percent geometry as bounded structural metrics", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      terminal.width = 1_280;
      terminal.height = 768;
      if (terminal.canvasElement) {
        terminal.canvasElement.width = 1_280;
        terminal.canvasElement.height = 768;
      }
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      expect(fake.callsFor("record_terminal_attach_observation")[2].args.observation).toMatchObject({
        elementWidth: 1_280,
        elementHeight: 768,
        screenWidth: 1_280,
        screenHeight: 768,
        canvasWidth: 1_280,
        canvasHeight: 768,
      });
    } finally {
      await rendered.cleanup();
    }
  });

  it("aborts with resizeFailed and never bottoms when the authoritative resize rejects", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.reject("pty_resize", "resize rejected");
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() => expect(observationStages(fake)).toEqual(["postWrite", "aborted"]));
      expect(fake.callsFor("record_terminal_attach_observation")[1].args.observation).toMatchObject({
        outcome: "resizeFailed",
        resizeConfirmed: false,
      });
      expect(terminal.bottomCalls).toBe(0);
    } finally {
      await rendered.cleanup();
    }
  });

  it("makes an out-of-order resize completion inert after a newer session owns the view", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const resizeA = deferred<void>();
    fake.onInvoke("pty_resize", (args) =>
      args.sessionId === SESSION_A ? resizeA.promise : undefined,
    );
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminalA = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminalA);
      expect(await frames!.flushFrame()).toBe(true);
      await waitFor(() => expect(fake.callsFor("pty_resize")).toHaveLength(1));

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(() => instanceFor(SESSION_B)).not.toThrow());
      const terminalB = instanceFor(SESSION_B);
      await releaseReplayAndFence(terminalB);
      await finishFrames(frames!);
      expect(terminalB.bottomCalls).toBe(1);

      const resizeCount = fake.callsFor("pty_resize").length;
      resizeA.resolve();
      await flushPromises();
      expect(fake.callsFor("pty_resize")).toHaveLength(resizeCount);
      expect(terminalA.bottomCalls).toBe(0);
      expect(terminalB.bottomCalls).toBe(1);
    } finally {
      await rendered.cleanup();
    }
  });

  it("cancels an older ordinary resize retry when a newer viewport succeeds", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("pty_resize", (args) => {
      if (args.cols === 90 && args.rows === 30) throw new Error("old viewport failed");
      return undefined;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);

      terminal.resize(90, 30);
      await flushPromises();
      terminal.resize(100, 31);
      await waitFor(() =>
        expect(
          fake.callsFor("pty_resize").some((call) => call.args.cols === 100 && call.args.rows === 31),
        ).toBe(true),
      );
      await new Promise((resolve) => setTimeout(resolve, 180));
      expect(
        fake.callsFor("pty_resize").filter((call) => call.args.cols === 90 && call.args.rows === 30),
      ).toHaveLength(1);
    } finally {
      await rendered.cleanup();
    }
  });

  it("bounds rejected observation retries, keeps stages unaccepted, and emits content-free diagnostics", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.onInvoke("record_terminal_attach_observation", () =>
      Promise.reject(new Error("test observation rejection")),
    );
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(6),
      );
      await waitFor(() =>
        expect(fake.callsFor("cancel_terminal_output_activation")).toHaveLength(1),
      );
      expect(observationStages(fake)).toEqual([
        "postWrite",
        "postWrite",
        "postWrite",
        "aborted",
        "aborted",
        "aborted",
      ]);
      expect(await frames!.flushFrame()).toBe(false);
      const diagnostics = warn.mock.calls
        .map((call: unknown[]) => call[0])
        .filter(
          (message: unknown): message is string =>
            typeof message === "string" && message.includes("event=attach_observation"),
        );
      expect(diagnostics).toEqual([
        `[terminal-snapshot] event=attach_observation stage=postWrite outcome=rejected ` +
          `sessionId=${SESSION_A} documentEpoch=${TEST_TERMINAL_DOCUMENT_EPOCH} ` +
          `attachGeneration=1 attempts=3`,
        `[terminal-snapshot] event=attach_observation stage=aborted outcome=rejected ` +
          `sessionId=${SESSION_A} documentEpoch=${TEST_TERMINAL_DOCUMENT_EPOCH} ` +
          `attachGeneration=1 attempts=3`,
      ]);
      expect(diagnostics.join(" ")).not.toMatch(
        /replayData|terminalBytes|prompt|command|workingDirectory|cwd|argv|environment|userText|error/i,
      );
    } finally {
      await rendered.cleanup();
    }
  });

  it("retains a rejected exact-owner detach and reconciles it before reattaching", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    let detachAttempts = 0;
    fake.onInvoke("detach_terminal_output", () => {
      detachAttempts += 1;
      return detachAttempts <= 3
        ? Promise.reject(new Error("test detach rejection"))
        : undefined;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      await releaseReplayAndFence(instanceFor(SESSION_A));
      await finishFrames(frames!);

      terminalStore.setActiveSessionForTests(null);
      await waitFor(() => expect(detachAttempts).toBe(4));
      expect(warn).toHaveBeenCalledWith(
        `[terminal-snapshot] event=exact_owner_cleanup kind=detach outcome=rejected ` +
          `sessionId=${SESSION_A} documentEpoch=${TEST_TERMINAL_DOCUMENT_EPOCH} ` +
          `attachGeneration=1 attempts=3`,
      );

      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() => expect(fake.callsFor("activate_terminal_output")).toHaveLength(2));
      expect(fake.callsFor("detach_terminal_output").slice(0, 4).map((call) => call.args)).toEqual(
        Array.from({ length: 4 }, () => ({
          sessionId: SESSION_A,
          documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
          attachGeneration: 1,
        })),
      );
      await releaseReplayAndFence(instanceFor(SESSION_A));
      await finishFrames(frames!);
    } finally {
      await rendered.cleanup();
    }
  });

  it("retains a rejected exact-owner cancel and reconciles it before a later attach", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const firstActivation = deferred<unknown>();
    let activationAttempts = 0;
    fake.onInvoke("activate_terminal_output", (args) => {
      activationAttempts += 1;
      return activationAttempts === 1
        ? firstActivation.promise
        : terminalActivationWire(args, { replayData: SNAP });
    });
    let cancelAttempts = 0;
    fake.onInvoke("cancel_terminal_output_activation", (args) => {
      if (args.sessionId !== SESSION_A || args.attachGeneration !== 1) {
        return undefined;
      }
      cancelAttempts += 1;
      return cancelAttempts <= 3
        ? Promise.reject(new Error("test cancel rejection"))
        : undefined;
    });
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("activate_terminal_output")).toHaveLength(1));
      terminalStore.setActiveSessionForTests(null);
      await waitFor(() => expect(cancelAttempts).toBe(3));
      expect(warn).toHaveBeenCalledWith(
        `[terminal-snapshot] event=exact_owner_cleanup kind=cancel outcome=rejected ` +
          `sessionId=${SESSION_A} documentEpoch=${TEST_TERMINAL_DOCUMENT_EPOCH} ` +
          `attachGeneration=1 attempts=3`,
      );

      terminalStore.setActiveSessionForTests(SESSION_A);
      await waitFor(() => expect(cancelAttempts).toBe(4));
      await waitFor(() => expect(fake.callsFor("activate_terminal_output")).toHaveLength(2));
      expect(
        fake
          .callsFor("cancel_terminal_output_activation")
          .filter((call) => call.args.sessionId === SESSION_A && call.args.attachGeneration === 1)
          .slice(0, 4)
          .map((call) => call.args),
      ).toEqual(
        Array.from({ length: 4 }, () => ({
          sessionId: SESSION_A,
          documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
          attachGeneration: 1,
        })),
      );
      await releaseReplayAndFence(instanceFor(SESSION_A));
      await finishFrames(frames!);
    } finally {
      await rendered.cleanup();
    }
  });

  it("times out a hung activation at five seconds and exact-cancels its owner", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] });
    const fake = new FakeTransport();
    setupTransport(fake);
    const activation = deferred<unknown>();
    fake.onInvoke("activate_terminal_output", () => activation.promise);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      await flushPromises();
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(1);
      expect(fake.callsFor("cancel_terminal_output_activation")).toHaveLength(0);

      await vi.advanceTimersByTimeAsync(4_999);
      expect(fake.callsFor("cancel_terminal_output_activation")).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(1);
      await flushPromises();
      expect(fake.lastCall("record_terminal_attach_observation")?.args.observation).toMatchObject({
        stage: "aborted",
        outcome: "timeout",
      });
      expect(fake.lastCall("cancel_terminal_output_activation")?.args).toEqual({
        sessionId: SESSION_A,
        documentEpoch: TEST_TERMINAL_DOCUMENT_EPOCH,
        attachGeneration: 1,
      });
    } finally {
      await rendered.cleanup();
      vi.useRealTimers();
    }
  });

  it("cancels a queued frame and disposes every entry resource on unmount", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
    const terminal = instanceFor(SESSION_A);
    await releaseReplayAndFence(terminal);
    expect(observationStages(fake)).toEqual(["postWrite"]);

    await rendered.cleanup();
    expect(terminal.disposed).toBe(true);
    expect(terminal.element?.isConnected).toBe(false);
    expect(await frames!.flushFrame()).toBe(false);
    expect(terminal.bottomCalls).toBe(0);
    expect(fake.callsFor("cancel_terminal_output_activation")).toHaveLength(1);
  });

  it("does not bottom ordinary live output after settlement or override user scroll-up", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      terminal.baseY = 8;
      terminal.bufferLength = 32;
      terminal.viewportY = 3;
      const bottoms = terminal.bottomCalls;
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: LIVE,
        sequence: 2,
      });
      expect(terminal.bottomCalls).toBe(bottoms);
      expect(terminal.viewportY).toBe(3);
    } finally {
      await rendered.cleanup();
    }
  });

  it("classifies a preserved attach selection source as reattach diagnostics", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A, "attach");
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(() => instanceFor(SESSION_A)).not.toThrow());
      const terminal = instanceFor(SESSION_A);
      await releaseReplayAndFence(terminal);
      await finishFrames(frames!);
      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(3),
      );
      for (const call of fake.callsFor("record_terminal_attach_observation")) {
        expect(call.args.observation).toMatchObject({ transitionKind: "reattach" });
      }
    } finally {
      await rendered.cleanup();
    }
  });

  it("scopes pty_output to the current window and never calls unmocked Tauri metadata", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    terminalStore.setActiveSessionForTests(SESSION_A);
    const rendered = renderWithFakeTransport(() => <TerminalView />, fake);
    try {
      await waitFor(() => expect(fake.listensFor("pty_output")).toHaveLength(1));
      expect(fake.listensFor("pty_output")[0].options).toEqual({
        scopeToCurrentWindow: true,
      });
      expect(fake.callsFor("terminal_output_document_epoch")).toHaveLength(1);
    } finally {
      await rendered.cleanup();
    }
  });
});
