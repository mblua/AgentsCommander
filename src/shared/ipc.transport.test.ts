// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "./testing/fake-transport";
import { baseSettings } from "./testing/ui-harness";

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
});
