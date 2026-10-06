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

function taskSnapshot(task = OLD_TASK, workgroupRoot = WG_ROOT) {
  return { workgroupRoot, task, taskTitle: task.match(/title: ([^\n]+)/)?.[1] ?? null,
    description: "Human description", status: "In progress\nIssue 2842", revision: "topic:4",
    statusRecord: { schemaVersion: 1 as const, kind: "status" as const, topicId: "topic", sequence: 4,
      requestId: null, baseRevision: "topic:3", recordedAt: "2026-10-06", author: null, status: "In progress\nIssue 2842" }, tailIncomplete: false };
}

function setupTransport(fake: FakeTransport, listSessions: () => unknown): void {
  fake.resolve("task_get_snapshot", taskSnapshot());
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

  it.each([
    ["same session", "list first"], ["same session", "save first"],
    ["same room sibling", "list first"], ["same room sibling", "save first"],
    ["other room", "list first"], ["other room", "save first"],
  ])("#1455 %s / %s retains the authoritative task after rebinding", async (target, order) => {
    const fake = new FakeTransport();
    const staleList = deferred<unknown>();
    const save = deferred<unknown>();
    let holdList = false;
    setupTransport(fake, () => holdList ? staleList.promise : [wgSession(OLD_TASK)]);
    fake.resolve("task_get_title", "Old title");
    fake.onInvoke("task_set_title", () => save.promise);
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      await startSave(view.root, "New title");
      const other = target === "other room";
      const id = target === "same session" ? SESSION_A : SESSION_B;
      fake.resolve("get_active_session", liveSelection(id, 2));
      holdList = true;
      await forceHydration(fake, 1);
      expect(headerTitle(view.root)).toBeNull();
      const rebound = target === "same session" ? wgSession(OLD_TASK) :
        other ? otherWorkgroupSession(OTHER_WG_TASK) : siblingSession(OLD_TASK);
      const authoritative = other ? taskSnapshot(OTHER_WG_TASK, "C:/Project/.ac/wg-2-other-team") : taskSnapshot(NEW_TASK);
      fake.resolve("task_get_snapshot", authoritative);
      if (order === "list first") { staleList.resolve([rebound]); await flush(12); }
      save.resolve({ workgroupRoot: WG_ROOT, task: "not authoritative" });
      await flush(12);
      if (order === "save first") { staleList.resolve([rebound]); await flush(12); }
      expect(terminalStore.activeSessionId).toBe(id);
      expect(terminalStore.activeWorkgroupTask).toBe(authoritative.task);
      expect(headerTitle(view.root)).toBe(authoritative.taskTitle);
      expect(terminalStore.bindingState).toBe("bound");
    } finally { view.cleanup(); }
  });

  it("refreshes a saved title without events, then accepts an external snapshot on reconnect", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.resolve("task_get_title", "Old title");
    fake.onInvoke("task_set_title", () => {
      fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK));
      return { workgroupRoot: WG_ROOT, task: "not authoritative" };
    });
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      await startSave(view.root, "New title"); await flush(12);
      expect(headerTitle(view.root)).toBe("New title");
      fake.resolve("task_get_snapshot", taskSnapshot(EXTERNAL_TASK));
      await forceHydration(fake, 1); await flush(12);
      expect(headerTitle(view.root)).toBe("External title");
    } finally { view.cleanup(); }
  });
});

describe("P4 snapshot ownership and reconciliation", () => {
  let cleanupDom: () => void;
  beforeEach(() => { cleanupDom = installBrowserDomStubs(); resetUiStoresForTests(); });
  afterEach(() => { cleanupDom(); resetUiStoresForTests(); vi.useRealTimers(); });

  it("attaches independently of a slow snapshot and rejects an older read after invalidation", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    const old = deferred<ReturnType<typeof taskSnapshot>>();
    fake.onInvoke("task_get_snapshot", () => old.promise);
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(terminalStore.bindingState).toBe("bound"));
      expect(view.root.textContent).toContain("Loading task…");
      await waitFor(() => expect(fake.calls.some(c => c.cmd === "activate_terminal_output")).toBe(true));
      fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK));
      terminalStore.invalidateTask(WG_ROOT); await flush(12);
      expect(headerTitle(view.root)).toBe("New title");
      old.resolve(taskSnapshot()); await flush(12);
      expect(headerTitle(view.root)).toBe("New title");
      expect(terminalStore.activeTaskSnapshot?.revision).toBe("topic:4");
    } finally { view.cleanup(); }
  });

  it.each(["C:/PROJECT/.ac/wg-1-dev-team", String.fromCharCode(92).repeat(2) + "?" + String.fromCharCode(92) + "C:/Project/.ac/wg-1-dev-team"])("accepts compatible normalized root %s", async workgroupRoot => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK, workgroupRoot));
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try { await waitFor(() => expect(headerTitle(view.root)).toBe("New title")); }
    finally { view.cleanup(); }
  });

  it("rejects component-prefix mismatches and treats invalid snapshot reads as errors", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK, WG_ROOT + "-other"));
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(view.root.textContent).toContain("Could not read the task."));
      expect(headerTitle(view.root)).toBeNull();
    } finally { view.cleanup(); }
  });

  it.each([false, true])("keeps committed Clean disabled through repeated read failure (committed error=%s)", async committedError => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.onInvoke("task_clean", () => {
      fake.onInvoke("task_get_snapshot", () => { throw new Error("read failed"); });
      if (committedError) throw new Error("task mutation already committed; do not repeat Clean");
      return { workgroupRoot: WG_ROOT, task: "not authoritative" };
    });
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      const clean = view.root.querySelector<HTMLButtonElement>('button[title="Clean TASK (reset for new topic)"]')!;
      click(clean); await flush();
      const confirm = document.querySelector<HTMLButtonElement>(".quit-confirm-btn-quit")!;
      click(confirm); await flush(16);
      expect(view.root.textContent).toContain("Clean was saved, but the updated task could not be read. Clean is disabled until the task can be read.");
      expect(headerTitle(view.root)).toBeNull(); expect(clean.disabled).toBe(true);
      terminalStore.invalidateTask(WG_ROOT); await flush(16);
      expect(clean.disabled).toBe(true);
      expect(fake.calls.filter(c => c.cmd === "task_clean")).toHaveLength(1);
      fake.resolve("task_get_snapshot", { ...taskSnapshot(NEW_TASK), status: null, revision: "new-topic:0", statusRecord: null });
      terminalStore.invalidateTask(WG_ROOT); await flush(16);
      expect(clean.disabled).toBe(false); expect(headerTitle(view.root)).toBe("New title");
    } finally { view.cleanup(); }
  });

  it("pre-Clean and event-before-completion reads cannot replace the post-completion read", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    const pre = deferred<ReturnType<typeof taskSnapshot>>();
    const mutation = deferred<unknown>();
    fake.onInvoke("task_clean", () => mutation.promise);
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      fake.onInvoke("task_get_snapshot", () => pre.promise);
      terminalStore.invalidateTask(WG_ROOT); await flush();
      click(view.root.querySelector<HTMLButtonElement>('button[title="Clean TASK (reset for new topic)"]')!);
      await flush(); click(document.querySelector<HTMLButtonElement>(".quit-confirm-btn-quit")!); await flush();
      terminalStore.invalidateTask(WG_ROOT); await flush();
      fake.resolve("task_get_snapshot", { ...taskSnapshot(NEW_TASK), status: null, revision: "clean:0" });
      const before = fake.calls.filter(c => c.cmd === "task_get_snapshot").length;
      mutation.resolve({ workgroupRoot: WG_ROOT, task: "wrong" }); await flush(16);
      expect(fake.calls.filter(c => c.cmd === "task_get_snapshot").length).toBeGreaterThan(before);
      expect(headerTitle(view.root)).toBe("New title");
      pre.resolve(taskSnapshot()); await flush(16);
      expect(headerTitle(view.root)).toBe("New title");
    } finally { view.cleanup(); }
  });

  it("title mutation preserves authoritative status/topic/revision and never invokes Clean", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.resolve("task_get_title", "Old title");
    fake.onInvoke("task_set_title", () => { fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK)); return { workgroupRoot: WG_ROOT, task: "wrong" }; });
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      await startSave(view.root, "New title"); await flush(16);
      expect(headerTitle(view.root)).toBe("New title");
      expect(terminalStore.activeTaskSnapshot?.statusRecord?.topicId).toBe("topic");
      expect(terminalStore.activeTaskSnapshot?.revision).toBe("topic:4");
      expect(terminalStore.activeTaskSnapshot?.status).toBe(taskSnapshot().status);
      expect(fake.calls.filter(c => c.cmd === "task_set_title")[0].args).toEqual({ sessionId: SESSION_A, title: "New title" });
      expect(fake.calls.filter(c => c.cmd === "task_clean" || c.cmd === "task_clean_at")).toHaveLength(0);
    } finally { view.cleanup(); }
  });

  it("locked reconnect rehydrates its ID, drops old generation reads and removes its listener", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    const old = deferred<ReturnType<typeof taskSnapshot>>();
    fake.onInvoke("task_get_snapshot", () => old.promise);
    const view = renderWithFakeTransport(() => <TerminalApp embedded lockedSessionId={SESSION_A} />, fake);
    try {
      await waitFor(() => expect(terminalStore.bindingState).toBe("bound"));
      fake.resolve("task_get_snapshot", taskSnapshot(NEW_TASK));
      fake.setConnectionState({ state: "disconnected", generation: 1 }); await flush();
      expect(terminalStore.activeTaskSnapshot).toBeNull();
      fake.setConnectionState({ state: "connected", generation: 2 }); await flush(16);
      expect(headerTitle(view.root)).toBe("New title");
      old.resolve(taskSnapshot()); await flush(16);
      expect(headerTitle(view.root)).toBe("New title");
      const count = fake.calls.filter(c => c.cmd === "task_get_snapshot").length;

      view.cleanup(); fake.setConnectionState({ state: "connected", generation: 3 }); await flush(16);
      expect(fake.calls.filter(c => c.cmd === "task_get_snapshot")).toHaveLength(count);
    } finally { view.cleanup(); }
  });
  it("manual/poll events only invalidate matching roots and sessions; payloads never replace snapshots", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    const pending = deferred<ReturnType<typeof taskSnapshot>>();
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(fake.listens.some(l => l.event === "workgroup_task_updated")).toBe(true));
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      fake.onInvoke("task_get_snapshot", () => pending.promise);
      const count = () => fake.callsFor("task_get_snapshot").length;
      const before = count();
      fake.emitFromBackend("workgroup_task_updated", { source: "manual", workgroupRoot: WG_ROOT + "-other", task: "wrong", taskTitle: "wrong" });
      fake.emitFromBackend("workgroup_task_updated", { source: "poll", workgroupRoot: WG_ROOT, sessionIds: [SESSION_B], task: "wrong", taskTitle: "wrong" });
      await flush(); expect(count()).toBe(before);
      fake.emitFromBackend("workgroup_task_updated", { source: "manual", workgroupRoot: "C:/PROJECT/.ac/wg-1-dev-team", task: "wrong", taskTitle: "wrong" });
      await flush(); expect(count()).toBeGreaterThan(before);
      expect(headerTitle(view.root)).toBe("Old title");
      expect(view.root.textContent).toContain("Refreshing task…");
      expect(document.querySelector('[role="tooltip"]')).toBeNull();
      fake.emitFromBackend("workgroup_task_updated", { source: "poll", workgroupRoot: WG_ROOT, sessionIds: [SESSION_A], task: "older payload", taskTitle: "older" });
      await flush();
      pending.resolve({ ...taskSnapshot(NEW_TASK), revision: "new-topic:1", statusRecord: { ...taskSnapshot().statusRecord, topicId: "new-topic", sequence: 1 } });
      await flush(16);
      expect(headerTitle(view.root)).toBe("New title");
      expect(terminalStore.activeTaskSnapshot?.revision).toBe("new-topic:1");
    } finally { view.cleanup(); }
  });

  it("ordinary Clean failure rereads without a committed guard or mutation retry", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [wgSession(OLD_TASK)]);
    fake.reject("task_clean", "permission denied");
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    try {
      await waitFor(() => expect(headerTitle(view.root)).toBe("Old title"));
      const before = fake.callsFor("task_get_snapshot").length;
      click(view.root.querySelector<HTMLButtonElement>('button[title="Clean TASK (reset for new topic)"]')!);
      await flush(); click(document.querySelector<HTMLButtonElement>(".quit-confirm-btn-quit")!); await flush(16);
      expect(view.root.textContent).toContain("permission denied");
      expect(view.root.textContent).not.toContain("Clean was saved");
      expect(terminalStore.cleanPending).toBe(false);
      expect(fake.callsFor("task_clean")).toHaveLength(1);
      expect(fake.callsFor("task_get_snapshot").length).toBeGreaterThan(before);
      expect(headerTitle(view.root)).toBe("Old title");
    } finally { view.cleanup(); }
  });

  it("does not read for empty cwd and drops responses after disposal", async () => {
    const fake = new FakeTransport(); setupTransport(fake, () => [session({ id: SESSION_A, workingDirectory: "" })]);
    const view = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await waitFor(() => expect(terminalStore.bindingState).toBe("bound"));
    expect(fake.callsFor("task_get_snapshot")).toHaveLength(0);
    view.cleanup(); resetUiStoresForTests();
    setupTransport(fake, () => [wgSession(OLD_TASK)]);
    const pending = deferred<ReturnType<typeof taskSnapshot>>(); fake.onInvoke("task_get_snapshot", () => pending.promise);
    const second = renderWithFakeTransport(() => <TerminalApp embedded />, fake);
    await waitFor(() => expect(fake.callsFor("task_get_snapshot").length).toBeGreaterThan(0));
    second.cleanup(); pending.resolve(taskSnapshot(NEW_TASK)); await flush(16);
    expect(terminalStore.activeTaskSnapshot).toBeNull();
  });

});
