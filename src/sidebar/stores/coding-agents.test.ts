import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { FALLBACK_CODING_AGENTS } from "../../shared/agent-presets";
import type { CodingAgentDefinition } from "../../shared/types";
import { codingAgentsStore } from "./coding-agents";

const CMD = "get_coding_agent_catalog";
const LIST_CMD = "list_reseedable_agent_commands";

function def(key: string): CodingAgentDefinition {
  return {
    key,
    label: key,
    description: `by ${key}`,
    color: "#123456",
    command: key,
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
  };
}

describe("codingAgentsStore (#769)", () => {
  let fake: FakeTransport;

  beforeEach(() => {
    codingAgentsStore.resetForTests();
    fake = new FakeTransport();
    __setTransportForTests(fake);
    // Default the Phase-2 reseedable fetch so the catalog-focused cases don't hit
    // an unhandled invoke; individual tests override it.
    fake.resolve(LIST_CMD, []);
  });

  afterEach(() => {
    codingAgentsStore.resetForTests();
    vi.restoreAllMocks();
  });

  it("is seeded synchronously with the fallback (never blank before a fetch)", () => {
    expect(codingAgentsStore.catalog()).toBe(FALLBACK_CODING_AGENTS);
    expect(codingAgentsStore.loaded()).toBe(false);
  });

  it("replaces the catalog with the fetched list on success", async () => {
    const fetched = [def("alpha"), def("beta")];
    fake.resolve(CMD, fetched);
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toEqual(fetched);
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("honors a valid empty Ok([]) verbatim — does NOT resurrect the fallback", async () => {
    fake.resolve(CMD, []);
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("keeps the fallback and does not throw when the transport fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.reject(CMD, "config-dir failure");
    // Must resolve (never reject into the mounting component).
    await expect(codingAgentsStore.ensureLoaded()).resolves.toBeUndefined();
    expect(codingAgentsStore.catalog()).toBe(FALLBACK_CODING_AGENTS);
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("dedups concurrent loads into a single invoke", async () => {
    fake.resolve(CMD, [def("alpha")]);
    await Promise.all([
      codingAgentsStore.ensureLoaded(),
      codingAgentsStore.ensureLoaded(),
      codingAgentsStore.ensureLoaded(),
    ]);
    expect(fake.callsFor(CMD).length).toBe(1);
  });

  it("does not refetch once loaded", async () => {
    fake.resolve(CMD, [def("alpha")]);
    await codingAgentsStore.ensureLoaded();
    await codingAgentsStore.ensureLoaded();
    expect(fake.callsFor(CMD).length).toBe(1);
  });

  it("loads the reseedable command set on success", async () => {
    fake.resolve(CMD, [def("claude")]);
    fake.resolve(LIST_CMD, ["claude", "codex", "opencode"]);
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.reseedableCommands()).toEqual(["claude", "codex", "opencode"]);
  });

  it("independent failure: catalog fails but the reseedable set still loads", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.reject(CMD, "catalog dead");
    fake.resolve(LIST_CMD, ["claude"]);
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toBe(FALLBACK_CODING_AGENTS); // fallback kept
    expect(codingAgentsStore.reseedableCommands()).toEqual(["claude"]);
  });

  it("independent failure: reseedable fails but the catalog still loads (set stays empty = no buttons)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.resolve(CMD, [def("claude")]);
    fake.reject(LIST_CMD, "reseedable dead");
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toEqual([def("claude")]);
    expect(codingAgentsStore.reseedableCommands()).toEqual([]);
  });
});
