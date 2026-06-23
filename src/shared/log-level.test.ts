// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// Stable mock fns shared across module re-evaluations (vi.resetModules clears
// the module cache so each test gets a fresh `wired` guard, but these closures
// — and the vi.mock registrations — persist).
const listenMock = vi.fn();
const getMock = vi.fn();
const setLogLevelMock = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => listenMock(...args),
}));
vi.mock("./ipc", () => ({
  SettingsAPI: { get: (...args: unknown[]) => getMock(...args) },
}));
vi.mock("./console-capture", () => ({
  setLogLevel: (...args: unknown[]) => setLogLevelMock(...args),
}));

type LevelEvent = { payload?: { level?: string } };

beforeEach(() => {
  vi.resetModules(); // fresh `wired` guard per test
  listenMock.mockReset();
  getMock.mockReset();
  setLogLevelMock.mockReset();
  listenMock.mockResolvedValue(() => {});
  getMock.mockResolvedValue({ logLevel: "debug" });
});

async function load(): Promise<() => Promise<void>> {
  return (await import("./log-level")).initLogLevelForWindow;
}

describe("initLogLevelForWindow (#612)", () => {
  it("applies the fetched level and registers the live listener", async () => {
    getMock.mockResolvedValue({ logLevel: "trace" });
    const init = await load();
    await init();
    expect(setLogLevelMock).toHaveBeenCalledWith("trace");
    expect(listenMock).toHaveBeenCalledWith(
      "log_level_changed",
      expect.any(Function)
    );
  });

  it("coerces a null persisted level to info", async () => {
    getMock.mockResolvedValue({ logLevel: null });
    const init = await load();
    await init();
    expect(setLogLevelMock).toHaveBeenCalledWith("info");
  });

  it("is idempotent — a second call does not re-fetch or re-register", async () => {
    const init = await load();
    await init();
    await init();
    expect(getMock).toHaveBeenCalledTimes(1);
    expect(listenMock).toHaveBeenCalledTimes(1);
  });

  it("tolerates SettingsAPI.get() rejection and still registers the listener", async () => {
    getMock.mockRejectedValue(new Error("no backend"));
    const init = await load();
    await expect(init()).resolves.toBeUndefined();
    expect(setLogLevelMock).not.toHaveBeenCalled();
    expect(listenMock).toHaveBeenCalledWith(
      "log_level_changed",
      expect.any(Function)
    );
  });

  it("tolerates listen() rejection without throwing", async () => {
    listenMock.mockRejectedValue(new Error("no event bus"));
    const init = await load();
    await expect(init()).resolves.toBeUndefined();
  });

  it("re-applies the level when a log_level_changed event arrives", async () => {
    let handler: ((e: LevelEvent) => void) | undefined;
    listenMock.mockImplementation(
      (_event: string, cb: (e: LevelEvent) => void) => {
        handler = cb;
        return Promise.resolve(() => {});
      }
    );
    const init = await load();
    await init();
    setLogLevelMock.mockClear();
    handler?.({ payload: { level: "warn" } });
    expect(setLogLevelMock).toHaveBeenCalledWith("warn");
  });

  it("coerces a missing event payload level to info", async () => {
    let handler: ((e: LevelEvent) => void) | undefined;
    listenMock.mockImplementation(
      (_event: string, cb: (e: LevelEvent) => void) => {
        handler = cb;
        return Promise.resolve(() => {});
      }
    );
    const init = await load();
    await init();
    setLogLevelMock.mockClear();
    handler?.({ payload: undefined });
    expect(setLogLevelMock).toHaveBeenCalledWith("info");
  });
});
