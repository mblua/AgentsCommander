// @vitest-environment jsdom
//
// #2297 phase 3 - Spec Board quit gate, consent modal and registration lock.
//
// The board is mounted for real; only the Tauri window API and the heavy
// mermaid renderer are stubbed. jsdom does not implement `inert`, so this file
// asserts the lock STATE (attribute, disabled textarea, status) and containment
// only - native pointer/keyboard refusal and unchanged docId/content/dirty need
// the permitted real-webview pass.
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ListenOptions } from "../shared/transport";

const tauri = vi.hoisted(() => ({
  label: "spec-board",
  closeRequested: null as
    | ((event: { preventDefault: () => void }) => unknown)
    | null,
  destroyCalls: 0,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    label: tauri.label,
    onCloseRequested: async (
      handler: (event: { preventDefault: () => void }) => unknown,
    ) => {
      tauri.closeRequested = handler;
      return () => {
        tauri.closeRequested = null;
      };
    },
    destroy: async () => {
      tauri.destroyCalls += 1;
    },
    close: async () => undefined,
    minimize: () => undefined,
    toggleMaximize: () => undefined,
  }),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: () => undefined,
    render: async () => ({ svg: "" }),
  },
}));

vi.mock("../shared/platform", () => ({ isTauri: true, isBrowser: false }));

import SpecBoardApp from "./App";
import { setSpecBoardStore } from "./stores/spec-board";
import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";

class SequencedTransport extends FakeTransport {
  readonly events: string[] = [];

  override async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    this.events.push(`invoke:${cmd}`);
    return super.invoke<T>(cmd, args);
  }

  override async listen<T>(
    event: string,
    callback: (payload: T) => void,
    options?: ListenOptions,
  ): Promise<() => void> {
    this.events.push(`listen:${event}`);
    return super.listen<T>(event, callback, options);
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((next, fail) => {
    resolve = next;
    reject = fail;
  });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  for (let pass = 0; pass < 24; pass += 1) {
    await Promise.resolve();
  }
}

function resetSpecBoardStore(): void {
  setSpecBoardStore({
    docId: null,
    repoRoot: null,
    path: null,
    fileKind: "mermaid",
    content: "",
    diagramSource: "",
    dirty: false,
    conflict: null,
    renderError: null,
    snapshots: [],
    versionIndex: 0,
    versionCount: 0,
    previewZoom: 1,
    previewOffset: { x: 0, y: 0 },
    lastExternalUpdateAt: null,
    showAskAgent: false,
  });
}

let fake: SequencedTransport;
let restoreTransport: () => void;
let cleanup: (() => void) | null = null;
let errorSpy: ReturnType<typeof vi.spyOn>;
let warnSpy: ReturnType<typeof vi.spyOn>;

async function mountBoard(): Promise<HTMLDivElement> {
  const root = document.createElement("div");
  document.body.append(root);
  const dispose = render(() => <SpecBoardApp />, root);
  cleanup = () => {
    dispose();
    root.remove();
  };
  await flush();
  return root;
}

const controls = (root: HTMLElement): HTMLElement =>
  root.querySelector<HTMLElement>(".spec-board-controls")!;
const editor = (): HTMLTextAreaElement =>
  document.querySelector<HTMLTextAreaElement>(
    '[data-ac-testid="specBoard.editor.textarea"]',
  )!;
const closeModal = (): HTMLElement | null =>
  document.querySelector(".spec-board-modal-overlay");

function clickByTestId(testId: string): void {
  const button = document.querySelector<HTMLButtonElement>(
    `[data-ac-testid="${testId}"]`,
  );
  if (!button) {
    throw new Error(`missing testid ${testId}`);
  }
  button.click();
}

function requestQuit(epoch: number, label = "spec-board"): void {
  fake.emitFromBackend("app_quit_requested", { epoch, label });
}

describe("SpecBoardApp quit gate (#2297)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSpecBoardStore();
    fake = new SequencedTransport();
    restoreTransport = __setTransportForTests(fake);
    fake.resolve("quit_gate_register", { status: "Registered" });
    fake.resolve("quit_gate_unregister", undefined);
    fake.resolve("quit_gate_resolve", undefined);
    fake.resolve("quit_gate_progress", undefined);
    fake.resolve("list_sessions", []);
    tauri.label = "spec-board";
    tauri.closeRequested = null;
    tauri.destroyCalls = 0;
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    restoreTransport();
    vi.useRealTimers();
    errorSpy.mockRestore();
    warnSpy.mockRestore();
    document.body.replaceChildren();
  });

  it("installs the scoped listeners before registering and unlocks on Registered", async () => {
    const board = await mountBoard();

    const events = fake.events;
    const requestListenerAt = events.indexOf("listen:app_quit_requested");
    const registerAt = events.indexOf("invoke:quit_gate_register");
    expect(requestListenerAt).toBeGreaterThanOrEqual(0);
    expect(registerAt).toBeGreaterThan(requestListenerAt);
    expect(fake.listensFor("app_quit_requested")[0].options).toEqual({
      scopeToCurrentWindow: true,
    });
    expect(fake.listensFor("app_quit_cancelled")[0].options).toEqual({
      scopeToCurrentWindow: true,
    });
    // The outcome fallback is deliberately UNSCOPED (Any) so a board refused
    // registration inside an active round still sees the terminal outcome.
    expect(fake.listensFor("app_quit_outcome")[0].options).toBeUndefined();

    expect(fake.callsFor("quit_gate_register")).toHaveLength(1);
    expect(fake.callsFor("quit_gate_register")[0].args).toEqual({});
    expect(controls(board).hasAttribute("inert")).toBe(false);
    expect(editor().disabled).toBe(false);
    expect(board.querySelector(".spec-board-gate-status")).toBeNull();
  });

  it("keeps the board inert while registration is unresolved and contains only the mutating controls", async () => {
    const gate = deferred<unknown>();
    fake.onInvoke("quit_gate_register", () => gate.promise);
    setSpecBoardStore({
      conflict: {} as never,
      showAskAgent: true,
      renderError: "containment-check",
    });
    const board = await mountBoard();

    const wrapper = controls(board);
    expect(wrapper.hasAttribute("inert")).toBe(true);
    expect(editor().disabled).toBe(true);
    expect(board.querySelector(".spec-board-gate-status")).not.toBeNull();

    const toolbar = wrapper.querySelector(".spec-board-toolbar")!;
    const toolbarLabels = Array.from(toolbar.querySelectorAll("button")).map(
      (button) => button.textContent,
    );
    expect(toolbarLabels).toContain("New");
    expect(toolbarLabels).toContain("Open");
    expect(wrapper.contains(wrapper.querySelector(".spec-board-conflict-banner"))).toBe(true);
    expect(wrapper.contains(editor())).toBe(true);
    expect(wrapper.contains(wrapper.querySelector(".spec-board-preview"))).toBe(true);
    expect(wrapper.contains(wrapper.querySelector(".spec-board-ask-agent"))).toBe(true);
    expect(wrapper.contains(wrapper.querySelector(".spec-board-error"))).toBe(true);
    expect(wrapper.contains(wrapper.querySelector(".spec-board-footer"))).toBe(true);
    // The titlebar is the first child of the container and stays outside.
    const titlebar = board.querySelector(".spec-board-titlebar")!;
    expect(board.querySelector(".spec-board-container")!.firstElementChild).toBe(titlebar);
    expect(wrapper.contains(titlebar)).toBe(false);

    gate.resolve({ status: "Registered" });
    await flush();
    expect(wrapper.hasAttribute("inert")).toBe(false);
    expect(editor().disabled).toBe(false);
    expect(board.querySelector(".spec-board-gate-status")).toBeNull();
  });

  it("stays locked on active-round InFlight and retries after the terminal outcome", async () => {
    let registrations = 0;
    fake.onInvoke("quit_gate_register", () => {
      registrations += 1;
      return registrations === 1
        ? { status: "InFlight", epoch: 4 }
        : { status: "Registered" };
    });
    const board = await mountBoard();
    expect(controls(board).hasAttribute("inert")).toBe(true);
    expect(editor().disabled).toBe(true);
    expect(fake.callsFor("quit_gate_register")).toHaveLength(1);

    fake.emitFromBackend("app_quit_outcome", {
      outcome: "Aborted",
      epoch: 4,
      reason: "timeout",
      unansweredLabels: ["spec-board"],
    });
    await flush();
    expect(fake.callsFor("quit_gate_register")).toHaveLength(2);
    expect(controls(board).hasAttribute("inert")).toBe(false);
    expect(editor().disabled).toBe(false);
  });

  it("shows the failed/retry state after a rejected registration and recovers on a bounded retry", async () => {
    let registrations = 0;
    fake.onInvoke("quit_gate_register", () => {
      registrations += 1;
      if (registrations === 1) {
        throw new Error("gate unavailable");
      }
      return { status: "Registered" };
    });
    const board = await mountBoard();
    expect(controls(board).hasAttribute("inert")).toBe(true);
    expect(board.querySelector(".spec-board-gate-status")!.textContent).toContain(
      "gate unavailable",
    );
    expect(document.querySelector(".spec-board-gate-status button")).not.toBeNull();

    await vi.advanceTimersByTimeAsync(1000);
    await flush();
    expect(fake.callsFor("quit_gate_register")).toHaveLength(2);
    expect(controls(board).hasAttribute("inert")).toBe(false);
  });

  it("resolves a clean board true without a modal", async () => {
    setSpecBoardStore({ dirty: false, docId: "doc-1", path: "/tmp/a.mmd", content: "" });
    await mountBoard();
    requestQuit(9);
    await flush();

    expect(fake.lastCall("quit_gate_resolve")!.args).toEqual({
      epoch: 9,
      consent: true,
    });
    expect(closeModal()).toBeNull();
  });

  it("filters foreign labels and stale epochs, and a newer request replaces the epoch", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    await mountBoard();

    requestQuit(5, "terminal-1");
    await flush();
    expect(closeModal()).toBeNull();

    requestQuit(5);
    await flush();
    expect(closeModal()).not.toBeNull();

    requestQuit(4);
    await flush();
    clickByTestId("specBoard.saveBeforeClose.cancel");
    await flush();
    expect(fake.lastCall("quit_gate_resolve")!.args).toEqual({
      epoch: 5,
      consent: false,
    });

    requestQuit(6);
    await flush();
    requestQuit(7);
    await flush();
    clickByTestId("specBoard.saveBeforeClose.cancel");
    await flush();
    expect(fake.lastCall("quit_gate_resolve")!.args).toEqual({
      epoch: 7,
      consent: false,
    });
  });

  it("saves, reports progress and consents without destroying the board during a round", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    fake.resolve("spec_board_save", { docId: "doc-1", path: "/tmp/a.mmd" });
    await mountBoard();
    requestQuit(10);
    await flush();
    expect(closeModal()).not.toBeNull();

    clickByTestId("specBoard.saveBeforeClose.save");
    await flush();

    expect(fake.callsFor("quit_gate_progress").map((call) => call.args)).toEqual([
      { epoch: 10, busy: true },
      { epoch: 10, busy: false },
    ]);
    expect(fake.lastCall("quit_gate_resolve")!.args).toEqual({
      epoch: 10,
      consent: true,
    });
    expect(tauri.destroyCalls).toBe(0);
    expect(closeModal()).toBeNull();
    expect(editor().disabled).toBe(false);
    expect(document.querySelector('[data-ac-testid="specBoard.titlebar.close"]')).not.toBeNull();
  });

  it("keeps the board and modal open when Discard consent is rejected, then retries", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    let resolves = 0;
    fake.onInvoke("quit_gate_resolve", () => {
      resolves += 1;
      if (resolves === 1) {
        throw new Error("epoch gone");
      }
      return undefined;
    });
    await mountBoard();
    requestQuit(11);
    await flush();

    clickByTestId("specBoard.saveBeforeClose.discard");
    await flush();
    expect(tauri.destroyCalls).toBe(0);
    expect(closeModal()).not.toBeNull();
    expect(document.querySelector(".spec-board-error")!.textContent).toContain("epoch gone");

    clickByTestId("specBoard.saveBeforeClose.discard");
    await flush();
    expect(fake.callsFor("quit_gate_resolve")).toHaveLength(2);
    expect(tauri.destroyCalls).toBe(1);
    expect(closeModal()).toBeNull();
  });

  it("keeps standalone titlebar X -> Save closing the board without a quit round", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    fake.resolve("spec_board_save", { docId: "doc-1", path: "/tmp/a.mmd" });
    await mountBoard();

    const closeEvent = { preventDefault: vi.fn() };
    await tauri.closeRequested!(closeEvent);
    await flush();
    expect(closeEvent.preventDefault).toHaveBeenCalled();
    expect(closeModal()).not.toBeNull();

    clickByTestId("specBoard.saveBeforeClose.save");
    await flush();
    expect(fake.callsFor("quit_gate_resolve")).toHaveLength(0);
    expect(tauri.destroyCalls).toBe(1);
  });

  it("keeps the modal on a save error and leaves a cancelled picker pending", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    fake.reject("spec_board_save", "disk full");
    await mountBoard();
    requestQuit(12);
    await flush();

    clickByTestId("specBoard.saveBeforeClose.save");
    await flush();
    expect(closeModal()).not.toBeNull();
    expect(document.querySelector(".spec-board-error")!.textContent).toContain("disk full");
    expect(fake.callsFor("quit_gate_resolve")).toHaveLength(0);
    expect(fake.callsFor("quit_gate_progress").map((call) => call.args)).toEqual([
      { epoch: 12, busy: true },
      { epoch: 12, busy: false },
    ]);

    setSpecBoardStore({ path: null, renderError: null });
    fake.resolve("spec_board_pick_save", null);
    clickByTestId("specBoard.saveBeforeClose.save");
    await flush();
    expect(closeModal()).not.toBeNull();
    expect(fake.callsFor("quit_gate_resolve")).toHaveLength(0);
  });

  it("closes the modal on a matching cancellation and ignores a late save consent", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    const save = deferred<unknown>();
    fake.onInvoke("spec_board_save", () => save.promise);
    await mountBoard();
    requestQuit(13);
    await flush();
    clickByTestId("specBoard.saveBeforeClose.save");
    await flush();

    fake.emitFromBackend("app_quit_cancelled", { epoch: 13, label: "spec-board" });
    await flush();
    expect(closeModal()).toBeNull();

    save.resolve({ docId: "doc-1", path: "/tmp/a.mmd" });
    await flush();
    expect(fake.callsFor("quit_gate_resolve")).toHaveLength(0);
    expect(tauri.destroyCalls).toBe(0);
    expect(editor().disabled).toBe(false);
  });

  it("does not destroy a dirty board when the window close arrives during a pending round", async () => {
    setSpecBoardStore({ dirty: true, docId: "doc-1", path: "/tmp/a.mmd", content: "x" });
    await mountBoard();
    requestQuit(14);
    await flush();

    const closeEvent = { preventDefault: vi.fn() };
    await tauri.closeRequested!(closeEvent);
    await flush();
    expect(closeEvent.preventDefault).toHaveBeenCalled();
    expect(tauri.destroyCalls).toBe(0);
    expect(closeModal()).not.toBeNull();
  });

  it("unregisters the gate on cleanup", async () => {
    await mountBoard();
    cleanup!();
    cleanup = null;
    await flush();
    expect(fake.callsFor("quit_gate_unregister")).toHaveLength(1);
  });
});
