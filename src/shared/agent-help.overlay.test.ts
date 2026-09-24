import { afterEach, describe, expect, it, vi } from "vitest";
import type { Transport } from "./transport";

// Each case imports a fresh module graph so loadAgentHelpOverlay's one-shot
// `warned` latch starts unset.
async function freshModules(invoke: Transport["invoke"]) {
  vi.resetModules();
  const ipc = await import("./ipc");
  const restore = ipc.__setTransportForTests({ invoke } as unknown as Transport);
  const mod = await import("./agent-help");
  return { mod, restore };
}

let restore: (() => void) | null = null;

afterEach(() => {
  restore?.();
  restore = null;
  vi.restoreAllMocks();
});

describe("loadAgentHelpOverlay", () => {
  it("a rejected invoke yields the empty overlay", async () => {
    vi.spyOn(console, "debug").mockImplementation(() => {});
    const fresh = await freshModules(() => Promise.reject(new Error("unknown command")));
    restore = fresh.restore;
    await expect(fresh.mod.loadAgentHelpOverlay()).resolves.toBe(fresh.mod.EMPTY_AGENT_HELP_OVERLAY);
  });

  it("the unavailable warning is logged at most once", async () => {
    const debug = vi.spyOn(console, "debug").mockImplementation(() => {});
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const fresh = await freshModules(() => Promise.reject(new Error("unknown command")));
    restore = fresh.restore;
    await fresh.mod.loadAgentHelpOverlay();
    await fresh.mod.loadAgentHelpOverlay();
    expect(debug).toHaveBeenCalledTimes(1);
    expect(error).not.toHaveBeenCalled();
  });

  it("a malformed payload is normalized to absent", async () => {
    const malformed: unknown = { local: 7, remote: "x", localError: [] };
    const fresh = await freshModules((() => Promise.resolve(malformed)) as Transport["invoke"]);
    restore = fresh.restore;
    await expect(fresh.mod.loadAgentHelpOverlay()).resolves.toEqual({
      local: null,
      remote: null,
      localError: null,
    });
  });
});
