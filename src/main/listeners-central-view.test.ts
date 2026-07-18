import { describe, it, expect, vi, beforeEach } from "vitest";
import type { SessionSelection } from "../shared/types";
import { liveSelection, SESSION_A, userLiveSelection, userNoneSelection } from "../shared/testing/session-selection";

type SwitchedPayload = SessionSelection;

// Capture the listener callbacks so cases can fire crafted payloads through the
// same code path the backend would use (mirrors listeners-home.test.ts).
const m = vi.hoisted(() => ({
  switchedCb: null as ((data: SwitchedPayload) => void) | null,
  attachCb: null as (() => void) | null,
  setAttached: vi.fn(() => Promise.resolve()),
}));

// isTauri=false so centralViewStore.closeDetachedResourceMonitor() is a no-op.
vi.mock("../shared/platform", () => ({ isTauri: false }));

vi.mock("../shared/ipc", () => ({
  onSessionSwitched: vi.fn((cb: (data: SwitchedPayload) => void) => {
    m.switchedCb = cb;
    return Promise.resolve(() => {});
  }),
  onResourceMonitorAttach: vi.fn((cb: () => void) => {
    m.attachCb = cb;
    return Promise.resolve(() => {});
  }),
  // centralViewStore imports SettingsAPI; stub the narrow setter it persists with
  // so we can assert (no) persistence.
  SettingsAPI: { setMainResourceMonitorAttached: m.setAttached },
}));

import { wireCentralViewListeners } from "./listeners-central-view";
import {
  centralViewStore,
  __resetCentralViewStoreForTests,
} from "./stores/centralView";

describe("wireCentralViewListeners (issue #587)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetCentralViewStoreForTests();
    m.switchedCb = null;
    m.attachCb = null;
  });

  it("session_switched with userInitiated=true and a real id reveals the terminal", async () => {
    centralViewStore.setInitialView("resourceMonitor");
    await wireCentralViewListeners();
    m.switchedCb!(userLiveSelection(SESSION_A));
    expect(centralViewStore.isResourceMonitor).toBe(false);
    expect(m.setAttached).toHaveBeenCalledWith(false);
  });

  // HIGH-1 regression guard: a NON-user switch must NOT flip away from RM — it
  // would persist `false` and corrupt the restored mainResourceMonitorAttached
  // choice (and yank the RM view away mid-use).
  it("restore session_switched leaves RM in place", async () => {
    centralViewStore.setInitialView("resourceMonitor");
    await wireCentralViewListeners();
    m.switchedCb!(liveSelection(SESSION_A));
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(m.setAttached).not.toHaveBeenCalled();
  });

  it("session_switched with userInitiated=false leaves RM in place", async () => {
    centralViewStore.setInitialView("resourceMonitor");
    await wireCentralViewListeners();
    m.switchedCb!(liveSelection(SESSION_A));
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(m.setAttached).not.toHaveBeenCalled();
  });

  it("session_switched with id=null leaves RM in place even when userInitiated=true", async () => {
    centralViewStore.setInitialView("resourceMonitor");
    await wireCentralViewListeners();
    m.switchedCb!(userNoneSelection());
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(m.setAttached).not.toHaveBeenCalled();
  });

  it("resource_monitor_attach pulls RM back into the central pane", async () => {
    centralViewStore.setInitialView("terminal");
    await wireCentralViewListeners();
    m.attachCb!();
    expect(centralViewStore.isResourceMonitor).toBe(true);
    expect(m.setAttached).toHaveBeenCalledWith(true);
  });
});
