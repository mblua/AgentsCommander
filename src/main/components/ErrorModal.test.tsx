// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";

// #3037 - the modal's IPC is desktop-only; every case runs as Tauri against
// a controllable DebugAPI and error_log_event listener.
const ipc = vi.hoisted(() => ({
  ping: null as (() => void) | null,
  unlisten: vi.fn(),
  onErrorLogEvent: vi.fn(),
  getErrorPopupMutes: vi.fn(),
  setErrorPopupMuted: vi.fn(),
  drainErrorLogs: vi.fn(),
}));

vi.mock("../../shared/platform", () => ({ isTauri: true, isBrowser: false }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ requestUserAttention: async () => {} }),
  UserAttentionType: { Critical: 1 },
}));
vi.mock("../../shared/ipc", () => ({
  DebugAPI: {
    getErrorPopupMutes: ipc.getErrorPopupMutes,
    setErrorPopupMuted: ipc.setErrorPopupMuted,
    drainErrorLogs: ipc.drainErrorLogs,
  },
  onErrorLogEvent: ipc.onErrorLogEvent,
}));

import ErrorModal from "./ErrorModal";
import {
  errorModalStore,
  __resetErrorModalStoreForTests,
} from "../stores/error-modal";
import type { ErrorLogEntry } from "../../shared/types";

const entry: ErrorLogEntry = {
  timestamp: "2026-06-13T00:00:00.000Z",
  level: "error",
  target: "test",
  message: "boom",
  typeId: "type-boom",
};

let cleanup: (() => void) | null = null;
const documentListeners: Array<() => void> = [];

const hop = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  ipc.ping = null;
  ipc.unlisten.mockReset();
  ipc.onErrorLogEvent.mockReset().mockImplementation(async (cb: () => void) => {
    ipc.ping = cb;
    return ipc.unlisten;
  });
  ipc.getErrorPopupMutes.mockReset().mockResolvedValue([]);
  ipc.setErrorPopupMuted.mockReset().mockResolvedValue([]);
  ipc.drainErrorLogs.mockReset().mockResolvedValue([]);
});

afterEach(() => {
  cleanup?.();
  cleanup = null;
  for (const stop of documentListeners.splice(0)) stop();
  __resetErrorModalStoreForTests();
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function mount(): void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  cleanup = render(() => <ErrorModal />, root);
}

function keydown(key: string, shiftKey = false): KeyboardEvent {
  const e = new KeyboardEvent("keydown", {
    key,
    shiftKey,
    cancelable: true,
    bubbles: true,
  });
  document.dispatchEvent(e);
  return e;
}

/** A document listener registered after the modal's, so only the modal's
 *  `stopImmediatePropagation` decides whether it ever sees the key. */
function collectKeys(): string[] {
  const seen: string[] = [];
  const listener = (e: KeyboardEvent) => seen.push(e.key);
  document.addEventListener("keydown", listener, true);
  documentListeners.push(() => document.removeEventListener("keydown", listener, true));
  return seen;
}

describe("ErrorModal key routing", () => {
  it("does not intercept keys while closed", () => {
    mount();
    const seen = collectKeys();
    const e = keydown("Tab");
    expect(e.defaultPrevented).toBe(false);
    expect(seen).toEqual(["Tab"]);
  });

  it("Escape dismisses while open", async () => {
    mount();
    errorModalStore.enqueue([entry]);
    await hop();
    expect(errorModalStore.open).toBe(true);

    const e = keydown("Escape");
    expect(e.defaultPrevented).toBe(true);
    expect(errorModalStore.open).toBe(false);
  });

  it("stops immediate propagation for Tab and unrelated keys while open", async () => {
    mount();
    const seen = collectKeys();
    errorModalStore.enqueue([entry]);
    await hop();
    expect(errorModalStore.open).toBe(true);

    keydown("Tab");
    keydown("x");
    expect(seen).toEqual([]);
  });

  it("wraps Tab focus inside the modal", async () => {
    mount();
    errorModalStore.enqueue([entry]);
    await hop();

    const message = document.querySelector<HTMLElement>(".error-modal-message");
    const dismiss = document.querySelector<HTMLButtonElement>(".error-modal-btn-dismiss");
    if (!message || !dismiss) throw new Error("error modal did not render");

    dismiss.focus();
    const e = keydown("Tab");
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(message);
  });
});

// ---------------------------------------------------------------------------
// #3037 - "hide future popups for this error type" checkbox.
// ---------------------------------------------------------------------------

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (err: unknown) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function mk(typeId: string, message: string): ErrorLogEntry {
  return { timestamp: "2026-10-16 05:00:00.000", level: "ERROR", target: "t", message, typeId };
}

const A1 = mk("A", "a1");
const A2 = mk("A", "a2");
const B1 = mk("B", "b1");
const STATUS = "Could not confirm popup preference.";

const muteBox = () => document.querySelector<HTMLInputElement>(".error-mute-field input")!;
const statusText = () => document.querySelector(".error-mute-status")?.textContent ?? "";
const dismissBtn = () => document.querySelector<HTMLButtonElement>(".error-modal-btn-dismiss")!;
const copyBtn = () => document.querySelector<HTMLButtonElement>(".error-modal-btn-copy")!;

async function mountWith(snapshot: string[], drained: ErrorLogEntry[]): Promise<void> {
  ipc.getErrorPopupMutes.mockResolvedValueOnce(snapshot);
  ipc.drainErrorLogs.mockResolvedValueOnce(drained);
  mount();
  await hop();
}

describe("ErrorModal popup mute (#3037)", () => {
  it("registers the listener, then snapshots, then drains; box enabled only after snapshot", async () => {
    const snap = deferred<string[]>();
    ipc.getErrorPopupMutes.mockReturnValueOnce(snap.promise);
    ipc.drainErrorLogs.mockResolvedValueOnce([A1]);
    errorModalStore.enqueue([B1]);
    mount();
    await hop();
    expect(ipc.onErrorLogEvent).toHaveBeenCalledTimes(1);
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(1);
    expect(ipc.drainErrorLogs).not.toHaveBeenCalled(); // serialized behind snapshot
    expect(muteBox().disabled).toBe(true);
    snap.resolve([]);
    await hop();
    expect(ipc.drainErrorLogs).toHaveBeenCalledTimes(1);
    expect(muteBox().disabled).toBe(false);
    expect(muteBox().checked).toBe(false);
    expect(errorModalStore.entries).toEqual([B1, A1]);
  });

  it("marks A by ID, B stays unmarked, unmark re-admits without replay", async () => {
    await mountWith([], [A1, B1]);
    ipc.setErrorPopupMuted.mockResolvedValueOnce(["A"]);
    muteBox().click();
    expect(muteBox().checked).toBe(false); // no optimistic success
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledWith("A", true);
    expect(muteBox().checked).toBe(true);
    expect(errorModalStore.current).toBe(A1); // current stays visible

    dismissBtn().click();
    expect(errorModalStore.current).toBe(B1);
    expect(muteBox().checked).toBe(false);

    // The backend drops future A; only B arrives.
    const B2 = mk("B", "b2");
    ipc.drainErrorLogs.mockResolvedValueOnce([B2]);
    ipc.ping!();
    await hop();
    expect(errorModalStore.entries).toEqual([A1, B1, B2]);

    dismissBtn().click();
    ipc.drainErrorLogs.mockResolvedValueOnce([A2]);
    ipc.ping!();
    await hop();
    dismissBtn().click();
    expect(errorModalStore.current).toBe(A2);
    expect(muteBox().checked).toBe(true);

    const drains = ipc.drainErrorLogs.mock.calls.length;
    ipc.setErrorPopupMuted.mockResolvedValueOnce([]);
    muteBox().click();
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenLastCalledWith("A", false);
    expect(muteBox().checked).toBe(false);
    expect(ipc.drainErrorLogs.mock.calls.length).toBe(drains); // no replay
  });

  it("a late drain of earlier A entries is still queued after A is muted", async () => {
    await mountWith([], [A1]);
    const drain = deferred<ErrorLogEntry[]>();
    ipc.drainErrorLogs.mockReturnValueOnce(drain.promise);
    ipc.ping!();
    await hop();
    ipc.setErrorPopupMuted.mockResolvedValueOnce(["A"]);
    muteBox().click();
    await hop();
    expect(ipc.setErrorPopupMuted).not.toHaveBeenCalled(); // behind the drain
    drain.resolve([A2]);
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledWith("A", true);
    expect(muteBox().checked).toBe(true);
    expect(errorModalStore.entries).toEqual([A1, A2]);
  });

  it("pings during a drain and a set lose nothing and keep queue order", async () => {
    await mountWith([], [A1]);
    const d1 = deferred<ErrorLogEntry[]>();
    const set = deferred<string[]>();
    const d2 = deferred<ErrorLogEntry[]>();
    const d3 = deferred<ErrorLogEntry[]>();
    ipc.drainErrorLogs
      .mockReturnValueOnce(d1.promise)
      .mockReturnValueOnce(d2.promise)
      .mockReturnValueOnce(d3.promise);
    ipc.setErrorPopupMuted.mockReturnValueOnce(set.promise);
    ipc.ping!();
    muteBox().click();
    ipc.ping!();
    ipc.ping!();
    await hop();
    expect(ipc.drainErrorLogs).toHaveBeenCalledTimes(2); // initial + d1
    const e1 = mk("C", "c1");
    const e2 = mk("C", "c2");
    const e3 = mk("D", "d3");
    d1.resolve([e1]);
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledTimes(1);
    expect(ipc.drainErrorLogs).toHaveBeenCalledTimes(2); // waits for set
    set.resolve(["A"]);
    await hop();
    d2.resolve([e2]);
    await hop();
    d3.resolve([e3]);
    await hop();
    expect(ipc.drainErrorLogs).toHaveBeenCalledTimes(4);
    expect(errorModalStore.entries).toEqual([A1, e1, e2, e3]);
  });

  it("blocks a second change in flight; Copy works; Dismiss during set applies only the captured ID", async () => {
    await mountWith([], [A1, B1]);
    const set = deferred<string[]>();
    ipc.setErrorPopupMuted.mockReturnValueOnce(set.promise);
    muteBox().click();
    await hop();
    expect(muteBox().disabled).toBe(true);
    muteBox().click(); // disabled: ignored
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledTimes(1);

    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    copyBtn().click();
    await hop();
    expect(write).toHaveBeenCalledWith("2026-10-16 05:00:00.000 [ERROR] t\na1");

    dismissBtn().click();
    expect(errorModalStore.current).toBe(B1);
    set.resolve(["A"]);
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledWith("A", true);
    expect(muteBox().checked).toBe(false); // B is not muted
    expect(muteBox().disabled).toBe(false);
  });

  it("a set whose response is lost reconciles from the snapshot", async () => {
    await mountWith([], [A1]);
    ipc.setErrorPopupMuted.mockRejectedValueOnce(new Error("lost"));
    ipc.getErrorPopupMutes.mockResolvedValueOnce(["A"]);
    vi.spyOn(console, "error").mockImplementation(() => {});
    muteBox().click();
    await hop();
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(2);
    expect(muteBox().checked).toBe(true);
    expect(statusText()).toBe("");
  });

  it("set and snapshot failing shows status and disables; a ping recovers without re-sending set", async () => {
    await mountWith([], [A1]);
    ipc.setErrorPopupMuted.mockRejectedValueOnce(new Error("set"));
    ipc.getErrorPopupMutes.mockRejectedValueOnce(new Error("snap"));
    vi.spyOn(console, "error").mockImplementation(() => {});
    muteBox().click();
    await hop();
    expect(muteBox().disabled).toBe(true);
    expect(muteBox().checked).toBe(false);
    expect(statusText()).toBe(STATUS);
    expect(document.getElementById("error-mute-help")?.textContent).toBe("Errors will still be logged.");
    expect(errorModalStore.entries).toEqual([A1]);
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(2); // no polling

    ipc.getErrorPopupMutes.mockResolvedValueOnce(["A"]);
    ipc.ping!();
    await hop();
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(3);
    expect(statusText()).toBe("");
    expect(muteBox().disabled).toBe(false);
    expect(muteBox().checked).toBe(true);
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledTimes(1);
  });

  it("a failed snapshot retries when current changes", async () => {
    ipc.getErrorPopupMutes.mockReset().mockRejectedValue(new Error("snap"));
    ipc.drainErrorLogs.mockResolvedValueOnce([A1, B1]);
    vi.spyOn(console, "error").mockImplementation(() => {});
    mount();
    await hop();
    // Initial snapshot + one retry when current went from none to A1.
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(2);
    expect(muteBox().disabled).toBe(true);
    expect(statusText()).toBe(STATUS);
    ipc.getErrorPopupMutes.mockResolvedValue([]);
    dismissBtn().click();
    await hop();
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(3);
    expect(muteBox().disabled).toBe(false);
    expect(statusText()).toBe("");
  });

  it("a drain failure keeps the queue and later pings still drain", async () => {
    await mountWith([], [A1]);
    vi.spyOn(console, "error").mockImplementation(() => {});
    ipc.drainErrorLogs.mockRejectedValueOnce(new Error("drain"));
    ipc.ping!();
    await hop();
    expect(errorModalStore.entries).toEqual([A1]);
    ipc.drainErrorLogs.mockResolvedValueOnce([B1]);
    ipc.ping!();
    await hop();
    expect(errorModalStore.entries).toEqual([A1, B1]);
  });

  it("an entry without typeId keeps Copy/Dismiss but disables the box with status", async () => {
    const odd = { ...A1, typeId: undefined } as unknown as ErrorLogEntry;
    await mountWith([], [odd]);
    expect(muteBox().disabled).toBe(true);
    expect(statusText()).toBe(STATUS);
    expect(copyBtn().disabled).toBe(false);
    dismissBtn().click();
    expect(errorModalStore.open).toBe(false);
  });

  it("a remount reads the muted set from the backend", async () => {
    await mountWith(["A"], [A1]);
    expect(muteBox().checked).toBe(true);
    cleanup?.();
    cleanup = null;
    expect(ipc.unlisten).toHaveBeenCalledTimes(1);
    await mountWith([], []);
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(2);
    expect(muteBox().checked).toBe(false);
  });

  it("unmount during listener registration unregisters it and starts no IPC", async () => {
    const reg = deferred<() => void>();
    ipc.onErrorLogEvent.mockReturnValueOnce(reg.promise);
    mount();
    cleanup?.();
    cleanup = null;
    reg.resolve(ipc.unlisten);
    await hop();
    expect(ipc.unlisten).toHaveBeenCalledTimes(1);
    expect(ipc.getErrorPopupMutes).not.toHaveBeenCalled();
    expect(ipc.drainErrorLogs).not.toHaveBeenCalled();
  });

  it("unmount during a drain ignores the late response and never starts the queued set", async () => {
    await mountWith([], [A1]);
    const drain = deferred<ErrorLogEntry[]>();
    ipc.drainErrorLogs.mockReturnValueOnce(drain.promise);
    ipc.ping!();
    muteBox().click();
    await hop();
    cleanup?.();
    cleanup = null;
    expect(ipc.unlisten).toHaveBeenCalledTimes(1);
    drain.resolve([B1]);
    await hop();
    expect(ipc.setErrorPopupMuted).not.toHaveBeenCalled();
    expect(errorModalStore.entries).toEqual([A1]);
  });

  it("unmount during a set ignores the late response", async () => {
    await mountWith([], [A1]);
    const set = deferred<string[]>();
    ipc.setErrorPopupMuted.mockReturnValueOnce(set.promise);
    muteBox().click();
    await hop();
    expect(ipc.setErrorPopupMuted).toHaveBeenCalledTimes(1);
    cleanup?.();
    cleanup = null;
    set.reject(new Error("late"));
    await hop();
    expect(ipc.getErrorPopupMutes).toHaveBeenCalledTimes(1); // no reconcile after disposal
    expect(errorModalStore.entries).toEqual([A1]);
  });

  it("renders the exact label/help/status wiring", async () => {
    await mountWith([], [A1]);
    const box = muteBox();
    expect(box.type).toBe("checkbox");
    expect(box.closest("label")?.textContent).toBe(
      "Hide future popups for this error type until the application closes",
    );
    expect(box.getAttribute("aria-describedby")).toBe("error-mute-help");
    const status = document.querySelector(".error-mute-status")!;
    expect(status.getAttribute("role")).toBe("status");
    expect(status.getAttribute("aria-live")).toBe("polite");
    expect(statusText()).toBe("");
    expect(document.getElementById("error-modal-title")?.textContent).toBe("Application Error");
    expect(document.querySelector(".error-modal-message")?.textContent).toBe("a1");
  });

  it("keeps the enabled box in the Tab trap, Space native, focus/counter/Escape intact", async () => {
    await mountWith([], [A1, B1]);
    const message = document.querySelector<HTMLElement>(".error-modal-message")!;
    expect(document.activeElement).toBe(dismissBtn());
    expect(document.querySelector(".error-modal-counter")?.textContent).toBe("1 of 2");

    keydown("Tab");
    expect(document.activeElement).toBe(message);
    keydown("Tab", true);
    expect(document.activeElement).toBe(dismissBtn());

    // The box is a member of the trap set: no forced jump from it.
    muteBox().focus();
    expect(keydown("Tab").defaultPrevented).toBe(false);
    expect(keydown("Tab", true).defaultPrevented).toBe(false);
    expect(keydown(" ").defaultPrevented).toBe(false);

    // In flight it is disabled, so it is unfocusable and out of the set.
    ipc.setErrorPopupMuted.mockReturnValueOnce(new Promise(() => {}));
    muteBox().click();
    await hop();
    expect(muteBox().disabled).toBe(true);
    dismissBtn().focus();
    keydown("Tab");
    expect(document.activeElement).toBe(message);

    keydown("Escape");
    expect(errorModalStore.current).toBe(B1);
  });
});
