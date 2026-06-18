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
});
