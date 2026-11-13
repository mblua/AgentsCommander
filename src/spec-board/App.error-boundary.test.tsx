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


const toolbar = vi.hoisted(() => ({ broken: true }));
vi.mock("./components/SpecBoardToolbar", () => ({
  default: () => {
    if (toolbar.broken) throw new Error("toolbar boom");
    return <div data-testid="toolbar-ok" />;
  },
}));

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


let fake: FakeTransport;
let restoreTransport: () => void;
let cleanup: (() => void) | undefined;
beforeEach(() => {
  vi.useFakeTimers();
  resetSpecBoardStore();
  toolbar.broken = true;
  tauri.closeRequested = null;
  fake = new FakeTransport();
  restoreTransport = __setTransportForTests(fake);
  fake.resolve("quit_gate_unregister", undefined);
  fake.resolve("list_sessions", []);
  vi.spyOn(console, "error").mockImplementation(() => {});
  vi.spyOn(console, "warn").mockImplementation(() => {});
});
afterEach(() => {
  cleanup?.();
  restoreTransport();
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.replaceChildren();
});
it("keeps guard, gate and stable inert controls through failure and recovery", async () => {
  const gate = deferred<unknown>();
  fake.onInvoke("quit_gate_register", () => gate.promise);
  const root = document.createElement("div");
  document.body.append(root);
  expect(() => { cleanup = render(() => <SpecBoardApp />, root); }).not.toThrow();
  await flush();
  const controls = root.querySelector<HTMLElement>(".spec-board-controls")!;
  expect(controls.textContent).toContain("Something went wrong in this window.");
  expect(tauri.closeRequested).not.toBeNull();
  expect(fake.callsFor("quit_gate_unregister")).toHaveLength(0);
  expect(controls.hasAttribute("inert")).toBe(true);
  gate.resolve({ status: "Registered" });
  await flush();
  expect(controls.hasAttribute("inert")).toBe(false);
  toolbar.broken = false;
  Array.from(controls.querySelectorAll("button"))
    .find((button) => button.textContent === "Try again")!.click();
  await flush();
  expect(root.querySelector(".spec-board-controls")).toBe(controls);
  expect(controls.querySelector('[data-testid="toolbar-ok"]')).not.toBeNull();
  expect(controls.textContent).not.toContain("Something went wrong in this window.");
  expect(controls.hasAttribute("inert")).toBe(false);
  expect(tauri.closeRequested).not.toBeNull();
  expect(fake.callsFor("quit_gate_unregister")).toHaveLength(0);
});
