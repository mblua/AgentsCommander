import { describe, it, expect, vi, beforeEach } from "vitest";

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

// #3367 — the web browser is a third central view that is never persisted.
describe("centralViewStore browser view", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetCentralViewStoreForTests();
  });

  it("toggleBrowser shows and hides the browser without persisting", () => {
    centralViewStore.toggleBrowser();
    expect(centralViewStore.isBrowser).toBe(true);
    expect(centralViewStore.isResourceMonitor).toBe(false);
    centralViewStore.toggleBrowser();
    expect(centralViewStore.view).toBe("terminal");
    expect(setAttached()).not.toHaveBeenCalled();
  });

  it("showBrowser from the Resource Monitor persists the detach", () => {
    centralViewStore.showResourceMonitor();
    setAttached().mockClear();
    centralViewStore.showBrowser();
    expect(centralViewStore.isBrowser).toBe(true);
    expect(setAttached()).toHaveBeenCalledTimes(1);
    expect(setAttached()).toHaveBeenCalledWith(false);
  });

  it("showResourceMonitor replaces the browser", () => {
    centralViewStore.showBrowser();
    centralViewStore.showResourceMonitor();
    expect(centralViewStore.isBrowser).toBe(false);
    expect(centralViewStore.isResourceMonitor).toBe(true);
  });
});
