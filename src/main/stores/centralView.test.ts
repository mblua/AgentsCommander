import { describe, it, expect, vi, beforeEach } from "vitest";

// isTauri=false so closeDetachedResourceMonitor() is a no-op and we never reach
// the @tauri-apps/api/webviewWindow dynamic import.
vi.mock("../../shared/platform", () => ({ isTauri: false }));
vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    setMainResourceMonitorAttached: vi.fn(() => Promise.resolve()),
  },
}));

import { centralViewStore, __resetCentralViewStoreForTests } from "./centralView";
import { SettingsAPI } from "../../shared/ipc";

const setAttached = () =>
  SettingsAPI.setMainResourceMonitorAttached as ReturnType<typeof vi.fn>;

describe("centralViewStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetCentralViewStoreForTests();
  });

  it("defaults to the terminal view", () => {
    expect(centralViewStore.view).toBe("terminal");
    expect(centralViewStore.isResourceMonitor).toBe(false);
  });

  it("showResourceMonitor flips to RM and persists true", () => {
    centralViewStore.showResourceMonitor();
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(setAttached()).toHaveBeenCalledTimes(1);
    expect(setAttached()).toHaveBeenCalledWith(true);
  });

  it("showTerminal from RM flips back and persists false", () => {
    centralViewStore.showResourceMonitor();
    setAttached().mockClear();
    centralViewStore.showTerminal();
    expect(centralViewStore.view).toBe("terminal");
    expect(setAttached()).toHaveBeenCalledTimes(1);
    expect(setAttached()).toHaveBeenCalledWith(false);
  });

  it("showTerminal early-returns when already terminal (no persist storm)", () => {
    // session_switched fires often; the guard must avoid re-persisting false.
    centralViewStore.showTerminal();
    expect(setAttached()).not.toHaveBeenCalled();
  });

  it("showResourceMonitor early-returns when already RM (idempotent)", () => {
    centralViewStore.showResourceMonitor();
    setAttached().mockClear();
    centralViewStore.showResourceMonitor();
    expect(setAttached()).not.toHaveBeenCalled();
  });

  it("toggleResourceMonitor alternates between the two views", () => {
    centralViewStore.toggleResourceMonitor();
    expect(centralViewStore.isResourceMonitor).toBe(true);
    centralViewStore.toggleResourceMonitor();
    expect(centralViewStore.isResourceMonitor).toBe(false);
  });

  it("setInitialView sets the signal WITHOUT persisting (restore-only)", () => {
    centralViewStore.setInitialView("resourceMonitor");
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(setAttached()).not.toHaveBeenCalled();
  });
});
