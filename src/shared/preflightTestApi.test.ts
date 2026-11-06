import { describe, expect, it, vi } from "vitest";
import { createPreflightTestAPI } from "./preflightTestApi";
import type { PreflightTestDraft } from "./preflightTypes";

type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

function fakeTransport(result: unknown = undefined) {
  const invoke = vi.fn<Invoke>(async () => result);
  const api = createPreflightTestAPI({
    invoke: invoke as <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
  });
  return { api, invoke };
}

const draft: PreflightTestDraft = {
  codingAgent: { command: "echo agent", timeoutSeconds: 30 },
  profiles: { A: { command: null, timeoutSeconds: null } },
};

describe("createPreflightTestAPI", () => {
  it("lists targets with exactly the agent id and replica paths", async () => {
    const rows = [{
      replicaPath: "r1", requestedProfile: "A", effectiveProfile: "A",
      fallbackApplied: false, running: false,
    }];
    const { api, invoke } = fakeTransport(rows);
    await expect(api.listTargets("agent", ["r1", "r2"])).resolves.toBe(rows);
    expect(invoke).toHaveBeenCalledWith("list_preflight_test_targets",
      { agentId: "agent", replicaPaths: ["r1", "r2"] });
  });

  it("omits the draft key on describe and start when no draft is given", async () => {
    const { api, invoke } = fakeTransport("result");
    await expect(api.describe("agent", "A", "r1")).resolves.toBe("result");
    await expect(api.start("agent", "A", "r1", "rev")).resolves.toBe("result");
    expect(invoke.mock.calls).toEqual([
      ["describe_preflight_target", { agentId: "agent", profileLetter: "A", replicaPath: "r1" }],
      ["start_preflight_test",
        { agentId: "agent", profileLetter: "A", replicaPath: "r1", settingsRevision: "rev" }],
    ]);
    for (const [, args] of invoke.mock.calls) expect(args).not.toHaveProperty("draft");
  });

  it("keeps an explicit null draft as null for the saved chain", async () => {
    const { api, invoke } = fakeTransport();
    await api.describe("agent", null, "r1", null);
    await api.start("agent", null, "r1", "rev", null);
    expect(invoke.mock.calls).toEqual([
      ["describe_preflight_target",
        { agentId: "agent", profileLetter: null, replicaPath: "r1", draft: null }],
      ["start_preflight_test", {
        agentId: "agent", profileLetter: null, replicaPath: "r1",
        settingsRevision: "rev", draft: null,
      }],
    ]);
  });

  it("forwards a provided draft unchanged for Profile cell and card calls", async () => {
    const { api, invoke } = fakeTransport();
    await api.describe("agent", "B", "r1", draft);
    await api.start("agent", null, "r2", "rev", draft);
    expect(invoke.mock.calls).toEqual([
      ["describe_preflight_target",
        { agentId: "agent", profileLetter: "B", replicaPath: "r1", draft }],
      ["start_preflight_test", {
        agentId: "agent", profileLetter: null, replicaPath: "r2",
        settingsRevision: "rev", draft,
      }],
    ]);
    expect(invoke.mock.calls[0][1]?.draft).toBe(draft);
  });

  it("gets and cancels a run by id and forwards results", async () => {
    const snapshot = { runId: "run-1" };
    const { api, invoke } = fakeTransport(snapshot);
    await expect(api.get("run-1")).resolves.toBe(snapshot);
    await expect(api.cancel("run-1")).resolves.toBe(snapshot);
    expect(invoke.mock.calls).toEqual([
      ["get_preflight_test", { runId: "run-1" }],
      ["cancel_preflight_test", { runId: "run-1" }],
    ]);
  });

  it("propagates transport errors as rejected promises", async () => {
    const invoke = vi.fn<Invoke>(async () => { throw new Error("boom"); });
    const api = createPreflightTestAPI({
      invoke: invoke as <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
    });
    await expect(api.listTargets("agent", [])).rejects.toThrow("boom");
    await expect(api.describe("agent", null, "r1")).rejects.toThrow("boom");
    await expect(api.start("agent", null, "r1", "rev", draft)).rejects.toThrow("boom");
    await expect(api.get("run-1")).rejects.toThrow("boom");
    await expect(api.cancel("run-1")).rejects.toThrow("boom");
  });
});
