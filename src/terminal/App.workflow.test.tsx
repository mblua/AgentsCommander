// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "./App";
import { terminalStore } from "./stores/terminal";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";
import type { TransportConnectionState, UnlistenFn } from "../shared/transport";
import type { Session } from "../shared/types";
import {
  TEST_TERMINAL_DOCUMENT_EPOCH,
  terminalActivationWire,
} from "../shared/testing/terminal-output";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import {
  liveSelection,
  initialSelection,
  dormantSelection,
  noneSelection,
  ROOT_SESSION,
  SESSION_A,
  SESSION_B,
  TEST_EPOCH,
  TEST_EPOCH_2,
  userLiveSelection,
} from "../shared/testing/session-selection";

vi.mock("../sidebar/stores/sessions", () => ({
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

const tauriWindow = vi.hoisted(() => ({
  destroy: vi.fn(() => Promise.resolve()),
  onCloseRequested: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => tauriWindow,
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "terminal" }),
}));

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  writes: unknown[];
  /** What is actually visible now: reset() clears it, write() appends. */
  screen: unknown[];
  resets: number;
  disposed: boolean;
  reset(): void;
  resizes: { cols: number; rows: number }[];
  emitData(data: string): void;
  emitResize(cols: number, rows: number): void;
  resize(cols: number, rows: number): void;
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
    disposed = false;
    resizes: { cols: number; rows: number }[] = [];
    private viewportY = 0;
    private baseY = 0;
    private dataHandlers = new Set<(data: string) => void>();
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
      this.disposed = true;
      this.dataHandlers.clear();
      this.resizeHandlers.clear();
    }

    write(data: unknown, callback?: () => void): void {
      if (data instanceof Uint8Array && data.length === 0) {
        callback?.();
        return;
      }
      this.writes.push(data);
      this.screen.push(data);
      // Real xterm fires the write callback; the #1283 admission settles its
      // replay/write gates from it.
      callback?.();
    }

    /** Real xterm RIS: clears the screen and the scrollback. */
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

    proposeDimensions(): { cols: number; rows: number } {
      return { ...fitViewport };
    }
  },
}));

vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    activate(): void {}
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
  fake.resolve(
    "get_active_session",
    sessions[0] ? liveSelection(sessions[0].id) : initialSelection(),
  );
  fake.onInvoke("list_sessions", () => sessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.resolve("terminal_output_document_epoch", TEST_TERMINAL_DOCUMENT_EPOCH);

  // #1363: the selection path attaches and seeds from the snapshot the attach
  // resolves to. The default payload is an empty snapshot at the terminal's
  // own size; the legacy getScreenSnapshot surface is never consulted.
  fake.onInvoke("activate_terminal_output", (args) => {
    const sessionId = String(args.sessionId);
    const instance = xterm.instances.find(
      (candidate) =>
        candidate.element?.parentElement?.getAttribute("data-ac-session-id") === sessionId,
    );
    return terminalActivationWire(args, {
      replayData: [],
      rows: instance?.rows ?? 24,
      cols: instance?.cols ?? 80,
    });
  });
  fake.resolve("detach_terminal_output", undefined);
  fake.resolve("cancel_terminal_output_activation", undefined);
  fake.resolve("record_terminal_attach_observation", undefined);
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

class TrackingTerminalTransport extends FakeTransport {
  readonly selectionUnlisten = vi.fn();
  readonly connectionUnlisten = vi.fn();
  selectionRegistrationGate: Promise<void> | null = null;

  override async listen<T>(
    event: string,
    callback: (payload: T) => void,
  ): Promise<UnlistenFn> {
    if (event === "session_switched" && this.selectionRegistrationGate) {
      await this.selectionRegistrationGate;
    }
    const unlisten = await super.listen(event, callback);
    return () => {
      if (event === "session_switched") this.selectionUnlisten();
      unlisten();
    };
  }

  override onConnectionState(
    callback: (state: TransportConnectionState) => void,
  ): UnlistenFn {
    const unlisten = super.onConnectionState(callback);
    return () => {
      this.connectionUnlisten();
      unlisten();
    };
  }
}

async function flushPromises(): Promise<void> {
  for (let pass = 0; pass < 8; pass += 1) {
    await Promise.resolve();
  }
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
    tauriWindow.destroy.mockClear();
    tauriWindow.onCloseRequested.mockClear();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    xterm.instances.length = 0;
    vi.useRealTimers();
  });

  it("wires active-session PTY input, prompt capture, and PTY output", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
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
          sessionId: SESSION_A,
          data: [104, 101, 108, 108, 111],
        })
      );

      terminal.emitData("\r");
      await waitFor(() =>
        expect(fake.lastCall("set_last_prompt")?.args).toEqual({
          id: SESSION_A,
          text: "hello",
        })
      );

      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(3),
      );
      await flushPromises();

      terminal.emitResize(100, 32);
      expect(fake.lastCall("pty_resize")?.args).toEqual({
        sessionId: SESSION_A,
        cols: 100,
        rows: 32,
      });

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: [111, 107],
        sequence: 1,
      });

      // The empty attach snapshot seeds nothing; the live chunk is the only
      // write.
      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([111, 107]);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("follows session_switched events through backend state", async () => {
    const sessions = [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
      session({
        id: SESSION_B,
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
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));

      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      expect(fake.callsFor("list_sessions").length).toBeGreaterThan(0);

      xterm.instances[1].emitData("z");
      await waitFor(() =>
        expect(fake.lastCall("pty_write")?.args).toEqual({
          sessionId: SESSION_B,
          data: [122],
        })
      );
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("seeds from the attach snapshot once without echoing snapshot resize to PTY", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
      }),
    ]);
    // The attach payload carries the PTY's reported dimensions.
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, {
        replayData: [83, 78, 65, 80],
        rows: 30,
        cols: 120,
      }),
    );

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("pty_resize").length).toBeGreaterThan(0));
      fake.clearCalls();

      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      // The snapshot resized xterm to the PTY's size in order to paint it, and
      // that resize is suppressed from reaching the PTY (it already knows).
      expect(terminal.resizes).toContainEqual({ cols: 120, rows: 30 });
      expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([83, 78, 65, 80]);
      expect(hasPtyResizeCall(fake, SESSION_A, 120, 30)).toBe(false);
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);

      // #973. The snapshot resized xterm to the PTY's size, and the re-fit then
      // puts xterm back to the tile's size. But the PTY was ALREADY told that
      // size — by the mount fit, before `fake.clearCalls()` — and it never moved
      // since. Re-sending it is exactly the redundant resize #973 removed.
      //
      // The assertion requires the end state: xterm is back at the fitted size,
      // and the PTY was not spoken to for nothing.
      await waitFor(() =>
        expect({ cols: terminal.cols, rows: terminal.rows }).toEqual({
          cols: fitViewport.cols,
          rows: fitViewport.rows,
        })
      );
      expect(fake.callsFor("pty_resize")).toHaveLength(0);
      expect(terminal.writes).toHaveLength(1);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("replays live output after the attach snapshot, in order, exactly once", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_architect",
      }),
    ]);
    // The attach snapshot predates the live chunk (S=0 < 1).
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, { replayData: [83, 78, 65, 80] }),
    );

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];

      // The attach snapshot renders first (the exact payload, after the seed
      // reset), then the post-snapshot delivery drains in order: nothing
      // duplicated, nothing missing.
      await waitFor(() => expect(terminal.writes).toHaveLength(1));
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: liveOutput,
        sequence: 1,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.screen[0] as Uint8Array)).toEqual([83, 78, 65, 80]);
      expect(Array.from(terminal.screen[1] as Uint8Array)).toEqual(liveOutput);
      // Every attach re-seeds, so the seed reset ran exactly once (plan 3.4.2).
      expect(terminal.resets).toBe(1);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("drops deliveries already covered by the attach snapshot", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_architect",
      }),
    ]);
    // The snapshot's screen already contains the live chunk (S=1).
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, {
        replayData: [83, 78, 65, 80, 76, 73, 86, 69],
        sequence: 1,
      }),
    );

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      // A snapshot-represented delivery is dropped by the watermark: it
      // appears exactly ONCE (already inside the snapshot).
      const liveOutput = [76, 73, 86, 69];
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: liveOutput,
        sequence: 1,
      });
      await flushPromises();
      expect(terminal.writes).toHaveLength(1);

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: [78, 69, 87],
        sequence: 2,
      });
      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.writes[1] as Uint8Array)).toEqual([78, 69, 87]);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("drops delayed live output already covered by the attach snapshot", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_architect",
      }),
    ]);
    fake.onInvoke("activate_terminal_output", (args) =>
      terminalActivationWire(args, {
        replayData: [83, 78, 65, 80, 76, 73, 86, 69],
        sequence: 1,
      }),
    );

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      const terminal = xterm.instances[0];
      await waitFor(() => expect(terminal.writes).toHaveLength(1));

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: [76, 73, 86, 69],
        sequence: 1,
      });

      await flushPromises();
      expect(terminal.writes).toHaveLength(1);

      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_A,
        data: [78, 69, 87],
        sequence: 2,
      });

      await waitFor(() => expect(terminal.writes).toHaveLength(2));
      expect(Array.from(terminal.writes[1] as Uint8Array)).toEqual([78, 69, 87]);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("re-attaches with exact-owner release compensation and never consults the legacy snapshot", async () => {
    const sessions = [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_architect",
      }),
      session({
        id: SESSION_B,
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_dev-webpage-ui",
      }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, sessions);
    // The legacy provider would happily answer: the activated path must never
    // ask it anything.
    fake.onInvoke("get_screen_snapshot", (args) => ({
      sessionId: String(args.sessionId),
      data: [99],
      rows: null,
      cols: null,
      sequence: 999,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      await waitFor(() => expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0));
      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(3),
      );
      await flushPromises();

      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      await waitFor(() => expect(xterm.instances).toHaveLength(2));
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);
      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(6),
      );
      await flushPromises();
      await new Promise((resolve) => setTimeout(resolve, 0));

      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 3));
      await waitFor(() => {
        const sessionOne = rendered.root.querySelector<HTMLElement>(
          '[data-ac-testid="terminal.session.11111111-1111-4111-8111-111111111111"]'
        );
        expect(sessionOne?.hidden).toBe(false);
      });
      await waitFor(() =>
        expect(fake.callsFor("record_terminal_attach_observation")).toHaveLength(9),
      );

      // Each selection attaches once and seeds from that attach; the legacy
      // snapshot surface stays untouched. A fully unwound owner detaches;
      // transition-gap ownership may also be cancelled exactly while local
      // observation acceptance unwinds.
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);
      expect(fake.callsFor("activate_terminal_output")).toHaveLength(3);
      const releases = [
        ...fake.callsFor("detach_terminal_output"),
        ...fake.callsFor("cancel_terminal_output_activation"),
      ];
      expect(new Set(releases.map((call) => call.args.sessionId))).toEqual(
        new Set([SESSION_A, SESSION_B]),
      );
      expect(releases.length).toBeLessThanOrEqual(4);
      const releaseKeys = [
        ...fake
          .callsFor("detach_terminal_output")
          .map((call) => `detach:${call.args.sessionId}:${call.args.attachGeneration}`),
        ...fake
          .callsFor("cancel_terminal_output_activation")
          .map((call) => `cancel:${call.args.sessionId}:${call.args.attachGeneration}`),
      ];
      expect(new Set(releaseKeys).size).toBe(releaseKeys.length);
      for (const release of releases) {
        expect(release.args.documentEpoch).toBe(TEST_TERMINAL_DOCUMENT_EPOCH);
        expect(release.args.attachGeneration).toEqual(expect.any(Number));
      }
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("recovers output dropped during the async session switch list gap via the attach snapshot", async () => {
    const sessionOne = session({
      id: SESSION_A,
      name: "wg-1-dev-team/architect",
      workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_architect",
    });
    const sessionTwo = session({
      id: SESSION_B,
      name: "wg-1-dev-team/dev-webpage-ui",
      workingDirectory: "C:\Project\.ac\wg-1-dev-team\__agent_dev-webpage-ui",
    });
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [sessionOne]);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      fake.clearCalls();

      const listSessions = deferred<Session[]>();
      fake.onInvoke("list_sessions", () => listSessions.promise);

      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      fake.emitFromBackend("pty_output", {
        sessionId: SESSION_B,
        data: [68, 82, 79, 80],
        sequence: 1,
      });

      // No terminal exists for B yet; the chunk must not recreate one.
      expect(xterm.instances).toHaveLength(1);
      expect(
        xterm.instances[0].writes.some((write) =>
          Array.from(write as Uint8Array).join(",") === "68,82,79,80"
        )
      ).toBe(false);

      // B's attach snapshot restores the dropped window once B binds.
      fake.onInvoke("activate_terminal_output", (args) => {
        const sessionId = String(args.sessionId);
        if (sessionId !== SESSION_B) {
          return terminalActivationWire(args, { replayData: [] });
        }
        return terminalActivationWire(args, {
          replayData: [82, 69, 80, 76, 65, 89],
          sequence: 1,
        });
      });
      listSessions.resolve([sessionOne, sessionTwo]);

      await waitFor(() => expect(xterm.instances).toHaveLength(2));

      const sessionTwoTerminal = xterm.instances[1];
      await waitFor(() => expect(sessionTwoTerminal.writes).toHaveLength(1));
      expect(Array.from(sessionTwoTerminal.writes[0] as Uint8Array)).toEqual([
        82, 69, 80, 76, 65, 89,
      ]);
      // The dropped raw chunk is never replayed on top of the seed: the seed
      // already contains it (its sequence is 1), and it was never retained.
      expect(sessionTwoTerminal.writes).toHaveLength(1);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("forwards automation input to the active PTY", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
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
          sessionId: SESSION_A,
          data: [115, 116, 97, 116, 117, 115, 13],
        })
      );
      await waitFor(() =>
        expect(fake.lastCall("set_last_prompt")?.args).toEqual({
          id: SESSION_A,
          text: "status",
        })
      );
      expect(input!.value).toBe("");
    } finally {
      await rendered.cleanupAsync();
    }
  });

  // #771 — the TASK panel must be hidden entirely for the Root Agent
  // (Agent's Commander); LAST PROMPT stays for every agent.
  it("hides the TASK panel for the Root Agent but keeps LAST PROMPT", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: ROOT_SESSION,
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
      await rendered.cleanupAsync();
    }
  });

  it("shows the TASK panel for a non-root agent", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
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
      await rendered.cleanupAsync();
    }
  });

  it("hides TASK on switch to the Root Agent and restores it on switch back", async () => {
    const sessions = [
      session({
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
        isRootAgent: false,
      }),
      session({
        id: ROOT_SESSION,
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
      fake.emitFromBackend("session_switched", userLiveSelection(ROOT_SESSION, 2));
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).toBeNull()
      );

      // Switch back to the non-root agent → TASK restored.
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 3));
      await waitFor(() =>
        expect(rendered.root.querySelector(".workgroup-task-panel")).not.toBeNull()
      );

      // LAST PROMPT is present throughout.
      expect(rendered.root.querySelector(".last-prompt-panel")).not.toBeNull();
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("clears every live route and metadata field on authoritative none", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({
        id: SESSION_A,
        name: "wg/architect",
        shell: "pwsh",
        workingDirectory: "C:\\Project",
        workgroupTask: "Ship it",
      }),
    ]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      fake.clearCalls();
      fake.emitFromBackend("session_switched", noneSelection(2));
      await waitFor(() =>
        expect(rendered.root.querySelector("[data-ac-testid='terminal.empty']")?.textContent)
          .toContain("No active session"),
      );
      expect(fake.callsFor("list_sessions")).toHaveLength(0);
      expect(terminalStore.activeSessionId).toBeNull();
      expect(terminalStore.activeSessionName).toBe("");
      expect(terminalStore.activeShell).toBe("");
      expect(terminalStore.activeWorkingDirectory).toBe("");
      expect(terminalStore.activeWorkgroupTask).toBeNull();
      xterm.instances[0].emitData("x");
      expect(fake.callsFor("pty_write")).toHaveLength(0);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("renders dormant wake guidance and performs no live PTY operation", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [
      session({ id: ROOT_SESSION, status: "active", isRootAgent: true }),
    ]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(xterm.instances).toHaveLength(1));
      fake.clearCalls();
      fake.emitFromBackend("session_switched", dormantSelection(ROOT_SESSION, 2, 17));
      await waitFor(() =>
        expect(rendered.root.textContent).toContain("Session exited. Wake it from the sidebar."),
      );
      xterm.instances[0].emitData("x");
      expect(fake.callsFor("list_sessions")).toHaveLength(0);
      expect(fake.callsFor("pty_write")).toHaveLength(0);
      expect(fake.callsFor("pty_resize")).toHaveLength(0);
      expect(fake.callsFor("get_screen_snapshot")).toHaveLength(0);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("suspends A synchronously while B metadata is pending", async () => {
    const rows = [
      session({ id: SESSION_A, status: "active" }),
      session({ id: SESSION_B, status: "running" }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, rows);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      const pending = deferred<Session[]>();
      fake.onInvoke("list_sessions", () => pending.promise);
      fake.clearCalls();
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      expect(terminalStore.activeSessionId).toBeNull();
      xterm.instances[0].emitData("stale");
      expect(fake.callsFor("pty_write")).toHaveLength(0);
      await waitFor(() =>
        expect(rendered.root.querySelector("[data-ac-testid='terminal.pending']")).not.toBeNull(),
      );
      pending.resolve(rows);
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_B));
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("rejects a stale list completion after a newer revision binds", async () => {
    const rows = [
      session({ id: SESSION_A, status: "active" }),
      session({ id: SESSION_B, status: "running" }),
    ];
    const fake = new FakeTransport();
    setupTerminalTransport(fake, rows);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      const stale = deferred<Session[]>();
      fake.onInvoke("list_sessions", () => stale.promise);
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      fake.onInvoke("list_sessions", () => rows);
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 3));
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      stale.resolve(rows);
      await flushPromises();
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
      expect(terminalStore.appliedRevision).toBe(3);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("makes a missing or exited live row unavailable without retaining A", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.onInvoke("list_sessions", () => [
        session({ id: SESSION_B, status: { exited: 9 } }),
      ]);
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      await waitFor(() => expect(terminalStore.bindingState).toBe("unavailable"));
      expect(terminalStore.activeSessionId).toBeNull();
      expect(rendered.root.textContent).toContain("Session unavailable");
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("safety-suspends a destroyed route without requerying or deriving a fallback", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.clearCalls();
      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      expect(terminalStore.activeSessionId).toBeNull();
      xterm.instances[0].emitData("stale");
      expect(fake.callsFor("pty_write")).toHaveLength(0);
      expect(fake.callsFor("list_sessions")).toHaveLength(0);
      expect(fake.callsFor("get_active_session")).toHaveLength(0);
      fake.emitFromBackend("session_switched", noneSelection(2));
      await waitFor(() => expect(terminalStore.selectionMode).toBe("none"));
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("clears routing on disconnect and equal-revision hydration rebinds once", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.setConnectionState({ state: "disconnected", generation: 0 });
      expect(terminalStore.activeSessionId).toBeNull();
      fake.setConnectionState({ state: "connected", generation: 1 });
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      const hydrationCalls = fake.callsFor("get_active_session").length;
      const listCalls = fake.callsFor("list_sessions").length;
      fake.emitFromBackend("session_switched", liveSelection(SESSION_A, 1));
      fake.setConnectionState({ state: "connected", generation: 1 });
      await flushPromises();
      expect(fake.callsFor("get_active_session")).toHaveLength(hydrationCalls);
      expect(fake.callsFor("list_sessions")).toHaveLength(listCalls);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("lets a new process epoch revision 0 replace revision 500 and rejects the retired epoch", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.resolve("get_active_session", liveSelection(SESSION_A, 500, TEST_EPOCH));
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.setConnectionState({ state: "disconnected", generation: 0 });
      fake.resolve("get_active_session", initialSelection(TEST_EPOCH_2));
      fake.setConnectionState({ state: "connected", generation: 1 });
      await waitFor(() => expect(terminalStore.selectionEpoch).toBe(TEST_EPOCH_2));
      expect(terminalStore.appliedRevision).toBe(0);
      expect(terminalStore.activeSessionId).toBeNull();

      fake.emitFromBackend(
        "session_switched",
        liveSelection(SESSION_A, 501, TEST_EPOCH),
      );
      await flushPromises();
      expect(terminalStore.selectionEpoch).toBe(TEST_EPOCH_2);
      expect(terminalStore.appliedRevision).toBe(0);
      expect(terminalStore.activeSessionId).toBeNull();
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("rejects an older-generation hydration even when its epoch was never applied", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const oldHydration = deferred<ReturnType<typeof liveSelection>>();
    let hydrationCalls = 0;
    fake.onInvoke("get_active_session", () => {
      hydrationCalls += 1;
      return hydrationCalls === 1
        ? oldHydration.promise
        : initialSelection(TEST_EPOCH_2);
    });
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(fake.callsFor("get_active_session")).toHaveLength(1));
      fake.setConnectionState({ state: "disconnected", generation: 0 });
      fake.setConnectionState({ state: "connected", generation: 1 });
      await waitFor(() => expect(terminalStore.selectionEpoch).toBe(TEST_EPOCH_2));
      oldHydration.resolve(liveSelection(SESSION_A, 500, TEST_EPOCH));
      await flushPromises();
      expect(terminalStore.selectionEpoch).toBe(TEST_EPOCH_2);
      expect(terminalStore.activeSessionId).toBeNull();
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("uses one capped busy-retry chain and restores only the current generation", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    let attempts = 0;
    fake.onInvoke("get_active_session", () => {
      attempts += 1;
      if (attempts < 7) throw "selectionCoordinatorBusy";
      return liveSelection(SESSION_A, 1);
    });
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsFor("get_active_session")).toHaveLength(1);
      for (const delay of [50, 100, 250, 500, 1000, 1000]) {
        await vi.advanceTimersByTimeAsync(delay - 1);
        expect(fake.callsFor("get_active_session")).toHaveLength(attempts);
        await vi.advanceTimersByTimeAsync(1);
      }
      expect(fake.callsFor("get_active_session")).toHaveLength(7);
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("does not resurrect a busy retry after a newer event cancels in-flight hydration", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const hydration = deferred<ReturnType<typeof liveSelection>>();
    fake.onInvoke("get_active_session", () => hydration.promise);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsFor("get_active_session")).toHaveLength(1);
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 2));
      hydration.reject("selectionCoordinatorBusy");
      await vi.advanceTimersByTimeAsync(5_000);
      expect(fake.callsFor("get_active_session")).toHaveLength(1);
      expect(terminalStore.appliedRevision).toBe(2);
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("cancels busy retry timers on disconnect, generation replacement, and unmount", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.reject("get_active_session", "selectionCoordinatorBusy");
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsFor("get_active_session")).toHaveLength(1);

    fake.setConnectionState({ state: "disconnected", generation: 0 });
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fake.callsFor("get_active_session")).toHaveLength(1);

    fake.setConnectionState({ state: "connected", generation: 1 });
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsFor("get_active_session")).toHaveLength(2);
      await rendered.cleanupAsync();
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fake.callsFor("get_active_session")).toHaveLength(2);
  });

  it("keeps missing and rejected live metadata safely unavailable", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.onInvoke("list_sessions", () => []);
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_B, 2));
      await waitFor(() => expect(terminalStore.bindingState).toBe("unavailable"));
      expect(terminalStore.activeSessionId).toBeNull();

      fake.onInvoke("list_sessions", () => { throw "list-failed"; });
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 3));
      await waitFor(() => expect(error).toHaveBeenCalled());
      expect(terminalStore.bindingState).toBe("unavailable");
      expect(terminalStore.activeSessionId).toBeNull();
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("keeps a locked detached route exact and ignores central selection events", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(
      () => <TerminalApp embedded lockedSessionId={SESSION_A} detached />,
      fake,
    );
    try {
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      fake.clearCalls();
      fake.emitFromBackend("session_switched", noneSelection(2));
      fake.emitFromBackend("session_switched", dormantSelection(SESSION_B, 3, 4));
      await flushPromises();
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
      expect(fake.callsFor("get_active_session")).toHaveLength(0);
      fake.emitFromBackend("session_destroyed", { id: SESSION_B });
      expect(tauriWindow.destroy).not.toHaveBeenCalled();
      fake.emitFromBackend("session_destroyed", { id: SESSION_A });
      await waitFor(() => expect(tauriWindow.destroy).toHaveBeenCalledOnce());
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("does not auto-select created rows and LastPrompt does not add a selection relist", async () => {
    const fake = new FakeTransport();
    setupTerminalTransport(fake, []);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.appliedRevision).toBe(0));
      fake.emitFromBackend("session_created", session({ id: SESSION_A, status: "running" }));
      await flushPromises();
      expect(terminalStore.activeSessionId).toBeNull();

      fake.onInvoke("list_sessions", () => [session({ id: SESSION_A, status: "running" })]);
      fake.clearCalls();
      fake.emitFromBackend("session_switched", userLiveSelection(SESSION_A, 1));
      await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
      expect(fake.callsFor("list_sessions")).toHaveLength(1);
    } finally {
      await rendered.cleanupAsync();
    }
  });

  it("drops a live-list completion after unmount", async () => {
    const fake = new FakeTransport();
    const list = deferred<Session[]>();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    fake.onInvoke("list_sessions", () => list.promise);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await waitFor(() => expect(terminalStore.bindingState).toBe("pending"));
    await rendered.cleanupAsync();
    list.resolve([session({ id: SESSION_A, status: "active" })]);
    await flushPromises();
    expect(terminalStore.activeSessionId).toBeNull();
    expect(terminalStore.bindingState).toBe("pending");
  });

  it("disposes central selection and connection listeners exactly once", async () => {
    const fake = new TrackingTerminalTransport();
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await waitFor(() => expect(terminalStore.activeSessionId).toBe(SESSION_A));
    await rendered.cleanupAsync();
    expect(fake.selectionUnlisten).toHaveBeenCalledOnce();
    expect(fake.connectionUnlisten).toHaveBeenCalledOnce();
  });

  it("immediately disposes a selection listener that resolves after terminal unmount", async () => {
    const fake = new TrackingTerminalTransport();
    const gate = deferred<void>();
    fake.selectionRegistrationGate = gate.promise;
    setupTerminalTransport(fake, [session({ id: SESSION_A, status: "active" })]);
    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await rendered.cleanupAsync();
    const restoreLateTransport = __setTransportForTests(fake);
    try {
      gate.resolve(undefined);
      await waitFor(() => expect(fake.selectionUnlisten).toHaveBeenCalledOnce());
      expect(fake.connectionUnlisten).not.toHaveBeenCalled();
      expect(fake.callsFor("get_active_session")).toHaveLength(0);
      expect(terminalStore.activeSessionId).toBeNull();
    } finally {
      restoreLateTransport();
    }
  });
});
