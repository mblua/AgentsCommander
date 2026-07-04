import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../testing/fake-transport";
import { __setTransportForTests } from "../ipc";

// #786: the settings store subscribes at MODULE INIT to
// `coding_agent_settings_updated` and re-fetches. We install the fake transport
// BEFORE importing the store so its subscription binds to the fake. The default
// transport is created lazily (only when used, and only when no test transport is
// set), so no real WebSocket/timer is ever constructed by this test.
describe("settingsStore #786 coding_agent_settings_updated", () => {
  let restore: (() => void) | undefined;

  afterEach(() => {
    restore?.();
    restore = undefined;
    vi.restoreAllMocks();
  });

  it("re-fetches settings when coding_agent_settings_updated fires", async () => {
    const fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    fake.resolve("get_settings", { soundsEnabled: true } as unknown);

    // The module-init subscription is skipped under vitest; call it explicitly
    // so it binds to the fake transport we just installed.
    const settings = await import("./settings");
    settings.__subscribeCodingAgentSettingsUpdates();
    const before = fake.callsFor("get_settings").length;

    fake.emitFromBackend("coding_agent_settings_updated", {
      op: "add",
      agentId: "agent_x",
    });

    await vi.waitFor(() => {
      expect(fake.callsFor("get_settings").length).toBe(before + 1);
    });
  });
});
