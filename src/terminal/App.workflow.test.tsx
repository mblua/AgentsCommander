// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
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
  emitData(data: string): void;
  emitResize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({
  instances: [] as FakeTerminalInstance[],
}));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
    writes: unknown[] = [];
    private dataHandlers = new Set<(data: string) => void>();
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
      for (const handler of Array.from(this.resizeHandlers)) {
        handler({ cols, rows });
      }
    }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn();
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    onContextLoss = vi.fn();
    dispose = vi.fn();
  },
}));

vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

function setupTerminalTransport(fake: FakeTransport, sessions = [session()]): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", sessions[0]?.id ?? null);
  fake.onInvoke("list_sessions", () => sessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
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
});
