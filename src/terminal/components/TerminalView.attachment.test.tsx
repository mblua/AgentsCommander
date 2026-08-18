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
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { terminalStore } from "../stores/terminal";
import { SESSION_A, SESSION_B } from "../../shared/testing/session-selection";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: number[][];
  screen: number[][];
  resets: number;
  disposed: boolean;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
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
    }
    write(data: unknown): void {
      const bytes = Array.from(data as Uint8Array);
      this.writes.push(bytes);
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
    fit = vi.fn();
    proposeDimensions(): { cols: number; rows: number } {
      return { cols: 80, rows: 24 };
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
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

async function flushPromises(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
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
      await flushPromises();
      await flushPromises();

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

      attach.resolve(null);
      await flushPromises();
      await flushPromises();

      expect(terminal.writes).toEqual([LIVE]);
      expect(terminal.resets).toBe(0);
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
      for (const instance of instancesFor(SESSION_B)) {
        expect(instance.writes).toEqual([SNAP]);
      }
    } finally {
      rendered.cleanup();
    }
  });
});
