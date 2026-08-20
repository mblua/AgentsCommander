// @vitest-environment jsdom
//
// #1283's two VERIFIED frontend defects, and only those. The delivery contract
// that PR #1312 built on top of them is gone (#1363); what remains here is the
// pair of fixes that attacked real bugs and that F keeps untouched:
//
//   1. unbounded Xterm/WebGL retention  -> the four-entry LRU registry,
//   2. writes reaching a hidden session -> the visibility filter at the single
//      writer.
//
// Plus the one attachment property this level can certify: a console switch
// costs exactly one detach and one attach, never a churn cycle.
//
// Deterministic: fake timers drive frame scheduling (rAF shimmed to
// setTimeout(0)) and the sustained load. No wall-clock waiting.
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
import { TERMINAL_RETENTION_LIMIT } from "./terminal-session-registry";

interface RecordedWrite {
  bytes: number[];
}

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: RecordedWrite[];
  screen: number[][];
  resets: number;
  disposed: boolean;
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
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

    write(data: unknown): void {
      const bytes = Array.from(data as Uint8Array);
      this.writes.push({ bytes });
      this.screen.push(bytes);
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

async function flushMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

/** Standard backend: every attach seeds a 4-byte screen at sequence 0, so the
 *  seed write is the deterministic "this attach settled" signal, and every
 *  live event after it (sequence >= 1) is past the watermark. The legacy
 *  snapshot provider answers, so a stray call to it would be visible. */
function installBackend(fake: FakeTransport, sessions: string[]): void {
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
  fake.onInvoke("activate_terminal_output", (args) => ({
    sessionId: String(args.sessionId),
    data: [83, 78, 65, 80],
    rows: 24,
    cols: 80,
    sequence: 0,
  }));
  fake.resolve("detach_terminal_output", undefined);
}

let currentFake: FakeTransport | null = null;
function emitPtyOutput(sessionId: string, sequence: number, data: number[]): void {
  currentFake?.emitFromBackend("pty_output", { sessionId, data, sequence });
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

function liveInstanceCount(): number {
  return xterm.instances.filter((instance) => !instance.disposed).length;
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

describe("TerminalView retention and visibility (#1283 fixes kept by #1363)", () => {
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

  // A console switch is the most frequent operation in this app. It must cost
  // exactly one detach of the old session and one attach of the new one — no
  // churn cycle, and no second attach of the same session.
  it("issues exactly one detach and one attach per console switch", async () => {
    const fake = new FakeTransport();
    installBackend(fake, [SESSION_A, SESSION_B]);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleReal();
      await waitFor(() => expect(instanceFor(SESSION_A).writes).toHaveLength(1));
      const attachesBefore = fake.callsFor("activate_terminal_output").length;
      const detachesBefore = fake.callsFor("detach_terminal_output").length;

      terminalStore.setActiveSessionForTests(SESSION_B);
      await waitFor(() => expect(instanceFor(SESSION_B).writes).toHaveLength(1));
      await flushMicrotasks();
      await flushMicrotasks();

      expect(fake.callsFor("activate_terminal_output")).toHaveLength(attachesBefore + 1);
      expect(fake.callsFor("detach_terminal_output")).toHaveLength(detachesBefore + 1);
      expect(fake.lastCall("activate_terminal_output")?.args).toEqual({
        sessionId: SESSION_B,
        includeHistory: true,
      });
      expect(fake.lastCall("detach_terminal_output")?.args).toEqual({
        sessionId: SESSION_A,
      });
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
    installBackend(fake, sessions);
    currentFake = fake;

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await settleFake();
      const chunk = new Array<number>(8 * 1024).fill(65);

      // 10 seconds of fake time, switching every 500 ms.
      for (let switchIndex = 0; switchIndex < 20; switchIndex += 1) {
        const sessionId = sessions[switchIndex % sessions.length];
        terminalStore.setActiveSessionForTests(sessionId);
        // Settle the attach deterministically: its seed write landing on the
        // visible instance means the seed was applied.
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

        // 16 chunks of 8 KiB per switch wave, every one past the seed's
        // sequence 0 so none is dropped by the watermark.
        for (let wave = 0; wave < 16; wave += 1) {
          emitPtyOutput(sessionId, wave + 1, chunk);
        }
        await vi.advanceTimersByTimeAsync(1);

        // Only the visible terminal received writes for this wave: no hidden
        // terminal ever reaches Terminal.write (criterion G).
        for (const [instance, count] of writesBefore) {
          if (instance === visible) {
            expect(instance.writes.length).toBeGreaterThan(count);
          } else {
            expect(instance.writes.length).toBe(count);
          }
        }

        // The registry never retains more than four live Xterm/WebGL pairs,
        // however many sessions the load cycles through.
        expect(liveInstanceCount()).toBeLessThanOrEqual(TERMINAL_RETENTION_LIMIT);

        await vi.advanceTimersByTimeAsync(500);
      }

      // Unmount: nothing scheduled remains; every instance disposed.
      rendered.cleanup();
      await vi.advanceTimersByTimeAsync(20_000);
      expect(xterm.instances.every((instance) => instance.disposed)).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      rendered.cleanup();
    }
  });
});
