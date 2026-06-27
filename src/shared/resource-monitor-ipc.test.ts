import { afterEach, describe, expect, it } from "vitest";
import { FakeTransport } from "./testing/fake-transport";
import {
  __setTransportForTests,
  ResourceMonitorAPI,
  WindowAPI,
} from "./ipc";

let restoreTransport: (() => void) | null = null;

const useFakeTransport = (): FakeTransport => {
  const fake = new FakeTransport();
  restoreTransport = __setTransportForTests(fake);
  return fake;
};

describe("resource monitor IPC", () => {
  afterEach(() => {
    if (restoreTransport) {
      restoreTransport();
      restoreTransport = null;
    }
  });

  it("opens the resource monitor window through the backend command", async () => {
    const fake = useFakeTransport();
    fake.resolve("open_resource_monitor_window", undefined);

    await WindowAPI.openResourceMonitor();

    expect(fake.lastCall("open_resource_monitor_window")?.args).toEqual({});
  });

  it("kills a group without sending process identifiers", async () => {
    const fake = useFakeTransport();
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "terminating",
      killedProcesses: [],
      quarantined: true,
      message: "queued",
      blockedBySecurity: false,
      finalized: false,
    });

    await ResourceMonitorAPI.killGroup({
      sessionId: "session-1",
      reason: "user",
    });

    const args = fake.lastCall("kill_resource_group")?.args;
    expect(args).toEqual({
      request: {
        sessionId: "session-1",
        reason: "user",
      },
    });
    expect(JSON.stringify(args).toLowerCase()).not.toContain("pid");
  });

  // #647 D: the result carries blockedBySecurity alongside the per-PID message;
  // the FE shows the AV-exclusion guidance only when the flag is set, never
  // replacing the per-PID detail.
  it("surfaces blockedBySecurity and the per-PID message from the kill result", async () => {
    const fake = useFakeTransport();
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "quarantined",
      killedProcesses: [],
      quarantined: true,
      message: "resource group cleanup incomplete: pid 4242: win32 error 5",
      blockedBySecurity: true,
      finalized: false,
    });

    const result = await ResourceMonitorAPI.killGroup({
      sessionId: "session-1",
      reason: "user",
    });

    expect(result.quarantined).toBe(true);
    expect(result.blockedBySecurity).toBe(true);
    expect(result.message).toContain("win32 error 5");
  });

  // #647 (Step 7): success keys off `finalized`, not `!quarantined`. The wire
  // result carries the flag so the FE can distinguish a verified teardown from a
  // `terminating` early-return that also reports `quarantined === false`.
  it("round-trips the finalized flag from the kill result", async () => {
    const fake = useFakeTransport();
    fake.resolve("kill_resource_group", {
      sessionId: "session-1",
      state: "terminated",
      killedProcesses: [],
      quarantined: false,
      message: "resource group terminated and verified",
      blockedBySecurity: false,
      finalized: true,
    });

    const result = await ResourceMonitorAPI.killGroup({
      sessionId: "session-1",
      reason: "user",
    });

    expect(result.finalized).toBe(true);
    expect(result.quarantined).toBe(false);
  });
});
