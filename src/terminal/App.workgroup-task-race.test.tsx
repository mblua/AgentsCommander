// @vitest-environment jsdom
//
// #1455 regression suite. `terminalStore.activeWorkgroupTask` is a pure cache with
// no periodic refresh, so its two asynchronous writers (a local TASK mutation and a
// `SessionAPI.list()` snapshot) must be sequenced instead of resolving
// last-write-wins.
//
// TASK.md is per-WORKGROUP, not per-session (`find_workgroup_task_path_for_cwd`,
// src-tauri/src/session/session.rs:242-256), so every session under one `wg-*` root
// shows the same file. Cases D, F, G and H are the 2x2 switch matrix that pins that:
// {same workgroup, different workgroup} x {snapshot lands first, save resolves first}.
//
// It drives the REAL TerminalApp -> reconcileSelection -> bindLive and the REAL
// WorkgroupTask -> saveTitle. The only thing mocked is the transport boundary.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import TerminalApp from "./App";
import { terminalStore } from "./stores/terminal";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { liveSelection, SESSION_A, SESSION_B } from "../shared/testing/session-selection";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    destroy: vi.fn(() => Promise.resolve()),
    onCloseRequested: vi.fn(() => Promise.resolve(() => undefined)),
  }),
}));

interface FakeTerminalInstance {
  cols: number;
  rows: number;
  element: HTMLElement | null;
  resize(cols: number, rows: number): void;
}

const xterm = vi.hoisted(() => ({ instances: [] as FakeTerminalInstance[] }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class implements FakeTerminalInstance {
    cols = 80;
    rows = 24;
    element: HTMLElement | null = null;
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
    dispose(): void {}
    write(_data: unknown, callback?: () => void): void {
      callback?.();
    }
    reset(): void {}
    scrollToBottom(): void {}
    paste(): void {}
    hasSelection(): boolean {
      return false;
    }
    getSelection(): string {
      return "";
    }
    clear(): void {}
    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
    }
    onData(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    onResize(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    onSelectionChange(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    attachCustomKeyEventHandler(): void {}
    registerLinkProvider(): { dispose: () => void } {
      return { dispose: () => undefined };
    }
    get buffer() {
      return { active: { cursorY: 0, viewportY: 0, length: 0, getLine: () => null } };
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
      this.terminal?.resize(88, 26);
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

vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false }));

const WG_ROOT = "C:\\Project\\.ac\\wg-1-dev-team";
const WG_CWD = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect";
const SIBLING_CWD = "C:\\Project\\.ac\\wg-1-dev-team\\__agent_dev-rust";
const OTHER_WG_CWD = "C:\\Project\\.ac\\wg-2-other-team\\__agent_dev-rust";
const OLD_TASK = "---\ntitle: Old title\n---\n\nbody\n";
const NEW_TASK = "---\ntitle: New title\n---\n\nbody\n";
const OTHER_WG_TASK = "---\ntitle: Other workgroup task\n---\n\nbody\n";
const EXTERNAL_TASK = "---\ntitle: External title\n---\n\nbody\n";

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

function wgSession(workgroupTask: string | null) {
  return session({
    id: SESSION_A,
    name: "wg-1-dev-team/architect",
    workingDirectory: WG_CWD,
    workgroupTask,
  });
}

/** Sibling agent of the SAME workgroup. It reads the same TASK.md as SESSION_A, so
 *  the backend can only ever give it identical `workgroupTask` content. */
function siblingSession(workgroupTask: string | null) {
  return session({
    id: SESSION_B,
    name: "wg-1-dev-team/dev-rust",
    workingDirectory: SIBLING_CWD,
    workgroupTask,
  });
}

/** Agent of a DIFFERENT workgroup, so a different TASK.md and different content. */
function otherWorkgroupSession(workgroupTask: string | null) {
  return session({
    id: SESSION_B,
    name: "wg-2-other-team/dev-rust",
    workingDirectory: OTHER_WG_CWD,
    workgroupTask,
  });
}

function setupTransport(fake: FakeTransport, listSessions: () => unknown): void {
  fake.resolve("get_settings", baseSettings());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.onInvoke("list_sessions", listSessions);
  fake.resolve("pty_write", undefined);
  fake.resolve("pty_resize", undefined);
  fake.resolve("set_last_prompt", undefined);
  fake.onInvoke("activate_terminal_output", (args) => ({
    sessionId: String(args.sessionId),
    data: [],
    rows: 24,
    cols: 80,
    sequence: 0,
  }));
  fake.resolve("detach_terminal_output", undefined);
}

function headerTitle(root: HTMLElement): string | null {
  return root.querySelector(".workgroup-task-title")?.textContent ?? null;
}

async function flush(times = 6): Promise<void> {
  for (let i = 0; i < times; i += 1) await Promise.resolve();
}

/** Click the pencil, type the new title, click Save. Returns once saveTitle is
 *  parked on its `await TaskAPI.setTitle(...)`. */
async function startSave(root: HTMLElement, title: string): Promise<void> {
  const editButton = root.querySelector<HTMLButtonElement>(
    'button.workgroup-task-action[title="Edit TASK title"]',
  );
  expect(editButton, "edit (pencil) button").toBeTruthy();
  expect(editButton!.disabled, "pencil must be enabled while bound").toBe(false);
  click(editButton!);

  await waitFor(() =>
    expect(root.querySelector(".workgroup-task-title-input")).toBeTruthy(),
  );
  const titleInput = root.querySelector<HTMLInputElement>(
    ".workgroup-task-title-input",
  )!;
  input(titleInput, title);

  const saveButton = root.querySelector<HTMLButtonElement>(
    "button.workgroup-task-title-btn.save",
  )!;
  expect(saveButton.disabled).toBe(false);
  click(saveButton);
  await flush();
}

/** Force a fresh connection generation while still connected. This is what
 *  `applyConnectionState` turns into requestHydration -> reconcileSelection. */
async function forceHydration(fake: FakeTransport, generation: number): Promise<void> {
  fake.setConnectionState({ state: "connected", generation });
  await flush();
}

describe("#1455 TASK header write ordering", () => {
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
    vi.useRealTimers();
  });

  it("CASE A: a stale list() snapshot resolving AFTER the save must not revert the header", async () => {
    const fake = new FakeTransport();
    const staleList = deferred<unknown>();
    let holdList = false;
    let heldCalls = 0;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      heldCalls += 1;
      return staleList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      holdList = true;
      await forceHydration(fake, 1);
      expect(heldCalls, "hydration must have issued a held list_sessions").toBe(1);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);

      staleList.resolve([wgSession(OLD_TASK)]);
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
      expect(terminalStore.activeSessionId).toBe(SESSION_A);
      expect(terminalStore.activeWorkingDirectory).toBe(WG_CWD);
      expect(terminalStore.bindingState).toBe("bound");
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE B: the same snapshot resolving BEFORE the save keeps the new title", async () => {
    const fake = new FakeTransport();
    const staleList = deferred<unknown>();
    let holdList = false;
    let heldCalls = 0;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      heldCalls += 1;
      return staleList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      holdList = true;
      await forceHydration(fake, 1);
      expect(heldCalls).toBe(1);

      staleList.resolve([wgSession(OLD_TASK)]);
      await flush(10);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE C: no hydration at all, the save sticks (baseline)", async () => {
    const fake = new FakeTransport();
    setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => ({
      workgroupRoot: WG_ROOT,
      task: NEW_TASK,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));
      await startSave(rendered.root, "New title");
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE D: switch to a DIFFERENT workgroup, snapshot first, the save must be dropped", async () => {
    const fake = new FakeTransport();
    const otherList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return otherList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);
      otherList.resolve([otherWorkgroupSession(OTHER_WG_TASK)]);
      await flush(10);
      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(headerTitle(rendered.root)).toBe("Other workgroup task");

      // wg-1's save now returns while a wg-2 session is bound. Different TASK.md,
      // so painting it here would display one workgroup's task under another's.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(OTHER_WG_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Other workgroup task"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE E: a snapshot taken AFTER the save still refreshes the header (the guard expires)", async () => {
    const fake = new FakeTransport();
    let listTask = OLD_TASK;
    setupTransport(fake, () => [wgSession(listTask)]);
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => ({
      workgroupRoot: WG_ROOT,
      task: NEW_TASK,
    }));

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");
      await flush(10);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);

      listTask = EXTERNAL_TASK;
      await forceHydration(fake, 1);
      await flush(10);

      expect(terminalStore.activeWorkgroupTask).toBe(EXTERNAL_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("External title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE F: switch to a SAME-workgroup sibling, snapshot first, the save must still paint", async () => {
    const fake = new FakeTransport();
    const siblingList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return siblingList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);
      // The sibling's snapshot was served before the write committed, so it carries
      // the pre-save content of the SHARED file.
      siblingList.resolve([siblingSession(OLD_TASK)]);
      await flush(10);
      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(headerTitle(rendered.root)).toBe("Old title");

      // The save returns. The sibling displays the very file that was edited, so
      // dropping this write would leave #1455's own symptom in place.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE G: switch to a SAME-workgroup sibling, save first, the stale sibling snapshot must lose", async () => {
    const fake = new FakeTransport();
    const siblingList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return siblingList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);

      // The save resolves while the store is unbound and the sibling's list is
      // still in flight.
      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      // Then the pre-save sibling snapshot lands. Same file, older content.
      siblingList.resolve([siblingSession(OLD_TASK)]);
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(NEW_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("New title"));
    } finally {
      rendered.cleanup();
    }
  });

  it("CASE H: switch to a DIFFERENT workgroup, save first, the new workgroup's snapshot must win", async () => {
    const fake = new FakeTransport();
    const otherList = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => {
      if (!holdList) return [wgSession(OLD_TASK)];
      return otherList.promise;
    });

    const setTitleCall = deferred<unknown>();
    fake.onInvoke("task_get_title", () => "Old title");
    fake.onInvoke("task_set_title", () => setTitleCall.promise);

    const rendered = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Old title"));

      await startSave(rendered.root, "New title");

      fake.resolve("get_active_session", liveSelection(SESSION_B, 2));
      holdList = true;
      await forceHydration(fake, 1);

      setTitleCall.resolve({ workgroupRoot: WG_ROOT, task: NEW_TASK });
      await flush(10);

      // wg-2's snapshot is a different TASK.md, so the wg-1 write must not suppress it.
      otherList.resolve([otherWorkgroupSession(OTHER_WG_TASK)]);
      await flush(10);

      expect(terminalStore.activeSessionId).toBe(SESSION_B);
      expect(terminalStore.activeWorkgroupTask).toBe(OTHER_WG_TASK);
      await waitFor(() => expect(headerTitle(rendered.root)).toBe("Other workgroup task"));
    } finally {
      rendered.cleanup();
    }
  });
});
