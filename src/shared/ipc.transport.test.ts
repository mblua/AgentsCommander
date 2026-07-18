// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "./testing/fake-transport";
import { baseSettings } from "./testing/ui-harness";
import { liveSelection, SESSION_A } from "./testing/session-selection";

describe("shared ipc transport seam", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("does not construct WebSocket transport on jsdom import before fake install", async () => {
    vi.resetModules();
    const websocketCtor = vi.fn(() => {
      throw new Error("real WebSocket should not be constructed");
    });
    vi.stubGlobal("WebSocket", websocketCtor);
    const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");

    const ipc = await import("./ipc");

    expect(websocketCtor).not.toHaveBeenCalled();
    expect(setTimeoutSpy).not.toHaveBeenCalled();

    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    const restore = ipc.__setTransportForTests(fake);
    try {
      await expect(ipc.SettingsAPI.get()).resolves.toMatchObject({
        defaultShell: "pwsh",
      });
    } finally {
      restore();
    }

    expect(fake.lastCall("get_settings")?.args).toEqual({});
    expect(websocketCtor).not.toHaveBeenCalled();
    expect(setTimeoutSpy).not.toHaveBeenCalled();
  });

  it("decodes selection hydration and events before invoking consumers", async () => {
    vi.resetModules();
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    const raw = liveSelection(SESSION_A);
    fake.resolve("get_active_session", raw);
    fake.setConnectionState({ state: "connected", generation: 3 });
    const restore = ipc.__setTransportForTests(fake);
    const callback = vi.fn();
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const decoded = await ipc.SessionAPI.getSelection();
      expect(decoded).toEqual(raw);
      expect(decoded).not.toBe(raw);

      const unlisten = await ipc.onSessionSwitched(callback);
      fake.emitFromBackend("session_switched", { ...raw, displayable: false });
      expect(callback).not.toHaveBeenCalled();
      fake.emitFromBackend("session_switched", raw);
      expect(callback).toHaveBeenCalledWith(expect.objectContaining({ id: SESSION_A }), 3);
      unlisten();
    } finally {
      restore();
      errorSpy.mockRestore();
    }
  });

  it("rejects malformed hydration and exposes local connection snapshots", async () => {
    vi.resetModules();
    const ipc = await import("./ipc");
    const fake = new FakeTransport();
    fake.resolve("get_active_session", { id: SESSION_A });
    const restore = ipc.__setTransportForTests(fake);
    const states: unknown[] = [];
    try {
      await expect(ipc.SessionAPI.getSelection()).rejects.toThrow(/Invalid session selection/);
      expect(ipc.getTransportConnectionState()).toEqual({ state: "connected", generation: 0 });
      const unlisten = await ipc.onTransportConnectionState((state) => states.push(state));
      fake.setConnectionState({ state: "disconnected", generation: 2 });
      expect(states).toEqual([{ state: "disconnected", generation: 2 }]);
      unlisten();
    } finally {
      restore();
    }
  });

  it("classifies only the exact coordinator busy string", async () => {
    const ipc = await import("./ipc");
    expect(ipc.isSelectionCoordinatorBusyError("selectionCoordinatorBusy")).toBe(true);
    for (const error of [
      "selectionCoordinatorUnavailable",
      "selectionCoordinatorBusy ",
      { message: "selectionCoordinatorBusy" },
      new Error("selectionCoordinatorBusy"),
      null,
    ]) {
      expect(ipc.isSelectionCoordinatorBusyError(error)).toBe(false);
    }
  });
});
