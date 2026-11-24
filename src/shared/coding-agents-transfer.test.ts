import { describe, expect, it } from "vitest";
import type { AgentConfig, CodingAgentProfilesConfig } from "./types";
import {
  TRANSFER_FORMAT,
  buildTransferFile,
  exportFileName,
  mergeTransferFile,
  parseTransferFile,
  transferHasEnvValues,
  uniqueAgentLabel,
} from "./coding-agents-transfer";

const NOW = new Date(2026, 10, 3, 4, 5, 6);

function agent(id: string, label: string, value = ""): AgentConfig {
  return {
    id,
    label,
    command: "claude",
    color: "#334155",
    envs: [{ key: "TOKEN", value, source: "user", enabled: true }],
    isolatedHome: false,
  };
}

function profiles(): CodingAgentProfilesConfig {
  return {
    schemaVersion: 2,
    profileSlots: { A: { label: "Main" }, B: { label: "Side" } },
    defaultProfileByAgent: { a1: "A" },
    profilesByAgent: {
      a1: {
        A: { id_hash: "h", enabled: true, command: "--fast", env: { K: "" }, notes: "n" },
      },
    },
    profileLabelsByAgent: { a1: { C: "Gamma" } },
  };
}

function validFile(): Record<string, unknown> {
  return JSON.parse(JSON.stringify(buildTransferFile([agent("a1", "Claude")], profiles(), NOW)));
}

function withAgent(patch: Record<string, unknown>): string {
  const file = validFile();
  const entries = file.agents as { agent: Record<string, unknown> }[];
  entries[0].agent = { ...entries[0].agent, ...patch };
  return JSON.stringify(file);
}

describe("coding agents transfer format", () => {
  it("names the export file with padded local time", () => {
    expect(exportFileName(NOW)).toBe("ac-coding-agents-exports_20261103_040506.json");
  });

  it("builds only the known fields, with cells, labels and used slots", () => {
    const a = { ...agent("a1", "Claude", "secret"), id_hash: "x", order: 3, extra: 1 };
    const file = buildTransferFile([a as AgentConfig], profiles(), NOW);
    const out = file.agents[0].agent as Record<string, unknown>;
    for (const key of ["id", "id_hash", "id_hash_debug", "order", "extra"]) {
      expect(out).not.toHaveProperty(key);
    }
    expect(file.agents[0].agent.envs[0].value).toBe("secret");
    expect(file.agents[0].profiles.A.cell).toEqual({
      enabled: true, command: "--fast", env: { K: "" }, notes: "n",
    });
    expect(file.agents[0].profiles.C).toEqual({ label: "Gamma" });
    expect(file).not.toHaveProperty("defaultProfileByAgent");
    expect(file.profileSlots).toEqual({ A: { label: "Main" }, C: { label: "" } });
    expect(file.format).toBe(TRANSFER_FORMAT);
    expect(file.exportedAt).toBe(NOW.toISOString());
  });

  it("detects environment variable values in agents and cells", () => {
    expect(transferHasEnvValues(buildTransferFile([agent("a1", "C", "v")], profiles(), NOW)))
      .toBe(true);
    const cellEnv = profiles();
    cellEnv.profilesByAgent.a1.A.env = { K: "v" };
    expect(transferHasEnvValues(buildTransferFile([agent("a1", "C")], cellEnv, NOW))).toBe(true);
    expect(transferHasEnvValues(buildTransferFile([agent("a1", "C")], profiles(), NOW)))
      .toBe(false);
  });

  it("round-trips, also with a leading BOM", () => {
    const built = buildTransferFile([agent("a1", "Claude", "v")], profiles(), NOW);
    const text = JSON.stringify(built);
    expect(parseTransferFile(text)).toEqual(built);
    expect(parseTransferFile("﻿" + text)).toEqual(built);
  });

  it.each([
    ["not json", "not valid JSON"],
    [JSON.stringify({ ...validFile(), format: "x" }), "format is not"],
    [JSON.stringify({ ...validFile(), formatVersion: 2 }), "formatVersion is not 1"],
    [JSON.stringify({ ...validFile(), agents: [] }), "contains no coding agents"],
    [JSON.stringify({ ...validFile(), agents: [null] }), "Agent 1: entry is not an object"],
    [JSON.stringify({ ...validFile(), agents: [[]] }), "Agent 1: entry is not an object"],
    [withAgent({ envs: [{ key: "K", value: "", source: "x", enabled: true }] }), "source"],
    [withAgent({ preflightTimeoutSeconds: 1.5 }), "preflightTimeoutSeconds"],
    [withAgent({ preflightTimeoutSeconds: 0 }), "preflightTimeoutSeconds"],
    [withAgent({ preflightTimeoutSeconds: 4294967296 }), "preflightTimeoutSeconds"],
    [JSON.stringify({ ...validFile(), profileSlots: [] }), "profileSlots is missing"],
    [JSON.stringify({ ...validFile(), profileSlots: { 1: { label: "" } } }), "profileSlots.1."],
    [JSON.stringify({ ...validFile(), profileSlots: { C: 7 } }), "profileSlots.C."],
    [JSON.stringify({ ...validFile(), profileSlots: { C: null } }), "profileSlots.C."],
    [JSON.stringify({ ...validFile(), profileSlots: { C: { label: 5 } } }), "profileSlots.C."],
  ])("rejects %#", (text, message) => {
    expect(() => parseTransferFile(text)).toThrow(message);
  });

  it("rejects a bad profile letter and a bad second agent", () => {
    const file = validFile();
    const entries = file.agents as Record<string, unknown>[];
    entries[0].profiles = { 1: {} };
    expect(() => parseTransferFile(JSON.stringify(file))).toThrow("letter is not A-Z");
    const two = validFile();
    two.agents = [(two.agents as unknown[])[0], { agent: { label: 1 } }];
    expect(() => parseTransferFile(JSON.stringify(two))).toThrow(/^Invalid import file: Agent 2: /);
  });

  it("drops unknown fields", () => {
    const parsed = parseTransferFile(withAgent({ id: "x", id_hash: "y", order: 1 }));
    const file = validFile();
    file.profileSlots = { C: { label: "c", extra: 1 } };
    expect(parsed.agents[0].agent).not.toHaveProperty("id");
    expect(parsed.agents[0].agent).not.toHaveProperty("id_hash");
    expect(parsed.agents[0].agent).not.toHaveProperty("order");
    expect(parseTransferFile(JSON.stringify(file)).profileSlots).toEqual({ C: { label: "c" } });
  });

  it("makes labels unique", () => {
    expect(uniqueAgentLabel("Free", new Set(["claude code"]))).toBe("Free");
    expect(uniqueAgentLabel("Claude Code", new Set(["claude code"]))).toBe("Claude Code (1)");
    const taken = new Set(["claude code", "claude code (1)"]);
    expect(uniqueAgentLabel("Claude Code", taken)).toBe("Claude Code (2)");
    taken.add("claude code (3)");
    expect(uniqueAgentLabel("Claude Code", taken)).toBe("Claude Code (2)");
    expect(uniqueAgentLabel("  ", new Set([""]))).toBe("  ");
  });

  it("merges with new ids, free labels, and untouched existing state", () => {
    const existing = [agent("a1", "Claude")];
    const before = profiles();
    const file = parseTransferFile(JSON.stringify(validFile()));
    file.agents.push(structuredClone(file.agents[0]));
    file.profileSlots.C = { label: "File C" };
    file.profileSlots.A = { label: "File A" };
    const agentsCopy = structuredClone(existing);
    const profilesCopy = structuredClone(before);
    const fileCopy = structuredClone(file);
    let n = 0;
    const out = mergeTransferFile(existing, before, file, () => `new${++n}`);

    expect(out.added.map((a) => [a.id, a.label])).toEqual([
      ["new1", "Claude (1)"],
      ["new2", "Claude (2)"],
    ]);
    expect(out.profiles.profilesByAgent.a1).toEqual(before.profilesByAgent.a1);
    expect(out.profiles.profileLabelsByAgent.a1).toEqual(before.profileLabelsByAgent.a1);
    expect(out.profiles.defaultProfileByAgent).toEqual(before.defaultProfileByAgent);
    expect(out.profiles.profilesByAgent.new1.A.command).toBe("--fast");
    expect(out.profiles.profileLabelsByAgent.new2).toEqual({ C: "Gamma" });
    expect(out.profiles.profileSlots.A).toEqual({ label: "Main" });
    expect(out.profiles.profileSlots.C).toEqual({ label: "File C" });
    expect(existing).toEqual(agentsCopy);
    expect(before).toEqual(profilesCopy);
    expect(file).toEqual(fileCopy);
  });
});
