// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { __setTransportForTests } from "./ipc";
import { registerShortcuts, unregisterShortcuts } from "./shortcuts";
import { voiceRecorder } from "./voice-recorder";
import { FakeTransport } from "./testing/fake-transport";
import {
  dormantSelection,
  liveSelection,
  noneSelection,
  SESSION_A,
} from "./testing/session-selection";

function press(key: string): void {
  document.dispatchEvent(new KeyboardEvent("keydown", {
    key,
    ctrlKey: true,
    shiftKey: true,
    bubbles: true,
  }));
}

describe("selection-aware shortcuts", () => {
  let restoreTransport: (() => void) | null = null;
  let handler: ((event: KeyboardEvent) => void) | null = null;

  beforeEach(() => {
    voiceRecorder.revokeLiveBinding();
  });

  afterEach(() => {
    if (handler) unregisterShortcuts(handler);
    handler = null;
    restoreTransport?.();
    restoreTransport = null;
    vi.restoreAllMocks();
  });

  it("allows close and voice for live selection", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_active_session", liveSelection(SESSION_A));
    fake.resolve("close_coordinator", { closed: true, workingCount: 0 });
    restoreTransport = __setTransportForTests(fake);
    const toggle = vi.spyOn(voiceRecorder, "toggle").mockImplementation(() => undefined);
    handler = registerShortcuts();
    press("w");
    await vi.waitFor(() => expect(fake.callsFor("close_coordinator")).toHaveLength(1));
    press("r");
    await vi.waitFor(() => expect(toggle).toHaveBeenCalledWith(SESSION_A));
  });

  it("allows dormant close but never starts voice", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_active_session", dormantSelection(SESSION_A, 1, 9));
    fake.resolve("close_coordinator", { closed: true, workingCount: 0 });
    restoreTransport = __setTransportForTests(fake);
    const toggle = vi.spyOn(voiceRecorder, "toggle").mockImplementation(() => undefined);
    handler = registerShortcuts();
    press("w");
    await vi.waitFor(() => expect(fake.callsFor("close_coordinator")).toHaveLength(1));
    press("r");
    await Promise.resolve();
    expect(toggle).not.toHaveBeenCalled();
  });

  it("does nothing for none", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_active_session", noneSelection());
    fake.resolve("close_coordinator", { closed: true, workingCount: 0 });
    restoreTransport = __setTransportForTests(fake);
    const toggle = vi.spyOn(voiceRecorder, "toggle").mockImplementation(() => undefined);
    handler = registerShortcuts();
    press("w");
    press("r");
    await Promise.resolve();
    expect(fake.callsFor("close_coordinator")).toHaveLength(0);
    expect(toggle).not.toHaveBeenCalled();
  });

  it("catches hydration and close failures without dispatching twice", async () => {
    const fake = new FakeTransport();
    fake.reject("get_active_session", "hydrate-failed");
    restoreTransport = __setTransportForTests(fake);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    handler = registerShortcuts();
    press("w");
    await vi.waitFor(() => expect(error).toHaveBeenCalledTimes(1));
    expect(fake.callsFor("get_active_session")).toHaveLength(1);

    fake.resolve("get_active_session", liveSelection(SESSION_A));
    fake.reject("close_coordinator", "close-failed");
    press("w");
    await vi.waitFor(() => expect(fake.callsFor("close_coordinator")).toHaveLength(1));
    await vi.waitFor(() => expect(error).toHaveBeenCalledTimes(2));
    expect(fake.callsFor("get_active_session")).toHaveLength(2);
    expect(fake.callsFor("close_coordinator")).toHaveLength(1);
  });
});
