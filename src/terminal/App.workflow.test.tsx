// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import type { PtyScreenSnapshot, Session } from "../shared/types";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: unknown[];
  resizes: { cols: number; rows: number }[];
  emitData(data: string): void;
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
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
    resizes: { cols: number; rows: number }[] = [];
    private dataHandlers = new Set<(data: string) => void>();
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
      this.dataHandlers.clear();
      this.resizeHandlers.clear();
    }

    write(data: unknown): void {
      this.writes.push(data);
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

    onData(handler: (data: string) => void): { dispose: () => void } {
      this.dataHandlers.add(handler);
      return { dispose: () => this.dataHandlers.delete(handler) };
    }

    onResize(
      handler: (size: { cols: number; rows: number }) => void
    ): { dispose: () => void } {
      this.resizeHandlers.add(handler);
      return { dispose: () => this.resizeHandlers.delete(handler) };
    }

    emitData(data: string): void {
      for (const handler of Array.from(this.dataHandlers)) {
        handler(data);
      }
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

vi.mock("../shared/platform", () => ({
  isTauri: true,
  isBrowser: false,
}));

function setupTerminalTransport(fake: FakeTransport, sessions = [session()]): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", sessions[0]?.id ?? null);
  fake.onInvoke("list_sessions", () => sessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("get_screen_snapshot", null);
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

function hasPtyResizeCall(
  fake: FakeTransport,
  sessionId: string,
  cols: number,
  rows: number
): boolean {
  return fake.callsFor("pty_resize").some((call) =>
    call.args.sessionId === sessionId &&
    call.args.cols === cols &&
    call.args.rows === rows
  );
}

describe("TerminalApp workflow", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    xterm.instances.length = 0;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
  });

  it("wires active-session PTY input, prompt capture, and PTY output", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];

      terminal.emitData("hello");
      await waitFor(() =>
        expect(fake.lastCall("pty_write")?.args).toEqual({
          sessionId: "session-1",
          data: [104, 101, 108, 108, 111],
        })
      );

      terminal.emitData("\r");
      await waitFor(() =>
        expect(fake.lastCall("set_last_prompt")?.args).toEqual({
          id: "session-1",
          text: "hello",
        })
      );

      terminal.emitResize(100, 32);
      expect(fake.lastCall("pty_resize")?.args).toEqual({
        sessionId: "session-1",
        cols: 100,
        rows: 32,
      });

      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: [111, 107],
      });

      expect(terminal.writes).toHaveLength(1);
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([111, 107]);
    } finally {
      rendered.cleanup();
    }
  });

  it("follows session_switched events through backend state", async () => {
    const sessions = [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
      session({
        id: "session-2",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui",
      }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, sessions);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));

      fake.clearCalls();
      fake.emitFromBackend("session_switched", { id: "session-2" });

      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      expect(fake.callsFor("list_sessions").length).toBeGreaterThan(0);

      xterm.instances[1].emitData("z");
      await waitFor(() =>
        expect(fake.lastCall("pty_write")?.args).toEqual({
          sessionId: "session-2",
          data: [122],
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("replays the first native snapshot once without echoing snapshot resize to PTY", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(fake.callsFor("get_screen_snapshot")[0]?.args).toEqual({
          sessionId: "session-1",
        })
      );

      await waitFor(() => expect(fake.callsFor("pty_resize").length).toBeGreaterThan(0));
      fake.clearCalls();

      snapshot.resolve({
        sessionId: "session-1",
        data: [83, 78, 65, 80],
        rows: 30,
        cols: 120,
        sequence: 0,
      });

      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      expect(terminal.resizes).toContainEqual({ cols: 120, rows: 30 });
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([83, 78, 65, 80]);
      expect(hasPtyResizeCall(fake, "session-1", 120, 30)).toBe(false);
      await waitFor(() =>
        expect(hasPtyResizeCall(fake, "session-1", fitViewport.cols, fitViewport.rows)).toBe(true)
      );
      expect(terminal.writes).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });

  it("replays buffered live output when the first native snapshot excludes it", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: liveOutput,
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(0);

      snapshot.resolve({
        sessionId: "session-1",
        data: [83, 78, 65, 80],
        rows: null,
        cols: null,
        sequence: 0,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([83, 78, 65, 80]);
      expect(Array.from(terminal.writes[1] as Uint8Array)).toEqual(liveOutput);
    } finally {
      rendered.cleanup();
    }
  });

  it("drops buffered live output when the first native snapshot already includes it", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: liveOutput,
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(0);

      snapshot.resolve({
        sessionId: "session-1",
        data: [83, 78, 65, 80, 76, 73, 86, 69],
        rows: null,
        cols: null,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([
        83, 78, 65, 80, 76, 73, 86, 69,
      ]);
    } finally {
      rendered.cleanup();
    }
  });

  it("drops delayed live output already covered by the first native snapshot", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      snapshot.resolve({
        sessionId: "session-1",
        data: [83, 78, 65, 80, 76, 73, 86, 69],
        rows: null,
        cols: null,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: [76, 73, 86, 69],
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(1);

      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: [78, 69, 87],
        sequence: 2,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.writes[1] as Uint8Array)).toEqual([78, 69, 87]);
    } finally {
      rendered.cleanup();
    }
  });

  it("writes unsequenced live output after the first native snapshot", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      snapshot.resolve({
        sessionId: "session-1",
        data: [83, 78, 65, 80],
        rows: null,
        cols: null,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: [79, 75],
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.writes[1] as Uint8Array)).toEqual([79, 75]);
    } finally {
      rendered.cleanup();
    }
  });

  it("does not request another snapshot when switching away and back to an existing terminal", async () => {
    const sessions = [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
      session({
        id: "session-2",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui",
      }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, sessions);
    fake.onInvoke("get_screen_snapshot", ({ sessionId }) => ({
      sessionId,
      data: sessionId === "session-1" ? [49] : [50],
      rows: null,
      cols: null,
      sequence: 0,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      fake.emitFromBackend("session_switched", { id: "session-2" });

      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(2));

      fake.emitFromBackend("session_switched", { id: "session-1" });

      await waitFor(() => {
        const sessionOne = rendered.root.querySelector<HTMLElement>(
          '[data-ac-testid="terminal.session.session-1"]'
        );
        expect(sessionOne?.hidden).toBe(false);
      });
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps live output when the native snapshot is unavailable later", async () => {
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: liveOutput,
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(0);

      snapshot.resolve(null);

      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      expect(terminal.writes).toHaveLength(1);
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual(liveOutput);
      const replayStatus = rendered.root.querySelector<HTMLDivElement>(
        '[data-ac-testid="terminal.replay-status.session-1"]'
      );
      expect(replayStatus?.hidden).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps live output when the native snapshot request fails later", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fake = new FakeTransport();
    const snapshot = deferred<PtyScreenSnapshot | null>();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    fake.onInvoke("get_screen_snapshot", () => snapshot.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(1));

      const terminal = xterm.instances[0];
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: "session-1",
        data: liveOutput,
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(0);

      snapshot.reject("missing parser");

      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      expect(terminal.writes).toHaveLength(1);
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual(liveOutput);
      const replayStatus = rendered.root.querySelector<HTMLDivElement>(
        '[data-ac-testid="terminal.replay-status.session-1"]'
      );
      expect(replayStatus?.hidden).toBe(true);
    } finally {
      rendered.cleanup();
      warn.mockRestore();
    }
  });

  it("recovers output dropped during the async session switch list gap by replaying a snapshot", async () => {
    const sessionOne = session({
      id: "session-1",
      name: "wg-1-dev-team/architect",
      workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
    });
    const sessionTwo = session({
      id: "session-2",
      name: "wg-1-dev-team/dev-webpage-ui",
      workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui",
    });
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [sessionOne]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      fake.clearCalls();

      const listSessions = deferred<Session[]>();
      fake.onInvoke("list_sessions", () => listSessions.promise);

      fake.emitFromBackend("session_switched", { id: "session-2" });
      fake.emitFromBackend("pty_output", {
        sessionId: "session-2",
        data: [68, 82, 79, 80],
        sequence: 1,
      });

      expect(xterm.instances).toHaveLength(1);
      expect(
        xterm.instances[0].writes.some((write) =>
          Array.from(write as Uint8Array).join(",") === "68,82,79,80"
        )
      ).toBe(false);

      fake.onInvoke("get_screen_snapshot", ({ sessionId }) =>
        sessionId === "session-2"
          ? {
              sessionId: "session-2",
              data: [82, 69, 80, 76, 65, 89],
              rows: null,
              cols: null,
              sequence: 1,
            }
          : null
      );
      listSessions.resolve([sessionOne, sessionTwo]);

      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      await waitFor(() =>
        expect(
          fake.callsFor("get_screen_snapshot").some((call) =>
            call.args.sessionId === "session-2"
          )
        ).toBe(true)
      );

      const sessionTwoTerminal = xterm.instances[1];
      await waitFor(() => expect(sessionTwoTerminal.writes).toHaveLength(1));
      expect(Array.from(sessionTwoTerminal.writes[0] as Uint8Array)).toEqual([
        82, 69, 80, 76, 65, 89,
      ]);
    } finally {
      rendered.cleanup();
    }
  });

  it("forwards automation input to the active PTY", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));

      const input = rendered.root.querySelector<HTMLTextAreaElement>(
        '[data-ac-testid="terminal.input"]'
      );
      expect(input?.getAttribute("data-ac-state")).toBe("ready");

      input!.value = "status\r";
      input!.dispatchEvent(new InputEvent("input", { bubbles: true }));

      await waitFor(() =>
        expect(fake.lastCall("pty_write")?.args).toEqual({
          sessionId: "session-1",
          data: [115, 116, 97, 116, 117, 115, 13],
        })
      );
      await waitFor(() =>
        expect(fake.lastCall("set_last_prompt")?.args).toEqual({
          id: "session-1",
          text: "status",
        })
      );
      expect(input!.value).toBe("");
    } finally {
      rendered.cleanup();
    }
  });

  // #771 — the TASK panel must be hidden entirely for the Root Agent
  // (Agent's Commander); LAST PROMPT stays for every agent.
  it("hides the TASK panel for the Root Agent but keeps LAST PROMPT", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: "root-1",
        name: "Agent's Commander",
        workingDirectory: "C:\\Project\\.ac\\ac-root-agent",
        isRootAgent: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).toBeNull()
      );
      expect(rendered.root.querySelector(".last-prompt-panel")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the TASK panel for a non-root agent", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
        isRootAgent: false,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).not.toBeNull()
      );
      expect(rendered.root.querySelector(".last-prompt-panel")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("hides TASK on switch to the Root Agent and restores it on switch back", async () => {
    const sessions = [
      session({
        id: "session-1",
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
        isRootAgent: false,
      }),
      session({
        id: "root-1",
        name: "Agent's Commander",
        workingDirectory: "C:\\Project\\.ac\\ac-root-agent",
        isRootAgent: true,
      }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, sessions);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      // Non-root is active on load → TASK visible.
      expect(rendered.root.querySelector(".workgroup-task-panel")).not.toBeNull();

      // Switch to the Root Agent → TASK hidden.
      fake.emitFromBackend("session_switched", { id: "root-1" });
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).toBeNull()
      );

      // Switch back to the non-root agent → TASK restored.
      fake.emitFromBackend("session_switched", { id: "session-1" });
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).not.toBeNull()
      );

      // LAST PROMPT is present throughout.
      expect(rendered.root.querySelector(".last-prompt-panel")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
