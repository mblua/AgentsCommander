// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests } from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";
import type { AgentUpdatePrompt, AgentUpdateResult } from "../shared/types";
import { agentUpdateStore, wireAgentUpdateListeners } from "./agent-update";

const [, setAgentUpdateStore] = agentUpdateStore;

const FAILING: AgentUpdateResult = {
  command: "claude",
  label: "Claude",
  ok: false,
  error: "exit code 1",
};

const PROMPT: AgentUpdatePrompt = { command: "claude", label: "Claude" };

describe("wireAgentUpdateListeners (#1327)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    setAgentUpdateStore({ inProgress: false, prompt: null });
  });

  afterEach(() => {
    toastStore.clear();
    setAgentUpdateStore({ inProgress: false, prompt: null });
    restore();
    vi.restoreAllMocks();
  });

  it("subscribe-then-snapshot dedups: event + snapshot with the same failing command toast exactly once", async () => {
    fake.resolve("get_agent_update_status", {
      inProgress: false,
      prompt: null,
      results: [FAILING],
    });
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    await Promise.resolve(); // flush the snapshot promise

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe(
      "Auto-update failed for Claude (claude): exit code 1"
    );
  });

  it("agent_updates_started sets inProgress; agent_updates_finished clears it and closes the prompt", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    expect(agentUpdateStore[0].inProgress).toBe(true);

    fake.emitFromBackend("agent_update_prompt", PROMPT);
    expect(agentUpdateStore[0].prompt).toEqual(PROMPT);

    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(agentUpdateStore[0].inProgress).toBe(false);
    expect(agentUpdateStore[0].prompt).toBeNull();
  });

  it("agent_update_prompt_closed clears the prompt but keeps the splash (F4)", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", null);

    expect(agentUpdateStore[0].prompt).toBeNull();
    expect(agentUpdateStore[0].inProgress).toBe(true);
  });

  it("snapshot restores a prompt emitted before wiring (F3) and the inProgress splash", async () => {
    fake.resolve("get_agent_update_status", {
      inProgress: true,
      prompt: PROMPT,
      results: [],
    });
    await wireAgentUpdateListeners();
    await Promise.resolve();

    expect(agentUpdateStore[0].inProgress).toBe(true);
    expect(agentUpdateStore[0].prompt).toEqual(PROMPT);
  });

  it("getStatus failure never breaks the live listeners (F8)", async () => {
    fake.reject("get_agent_update_status", "boom");
    const unlisteners = await wireAgentUpdateListeners();
    expect(unlisteners).toHaveLength(4);

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    expect(toastStore.items).toHaveLength(1);
  });
});
