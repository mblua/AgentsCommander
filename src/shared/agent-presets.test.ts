import { describe, expect, it } from "vitest";
import { FALLBACK_CODING_AGENTS, definitionToSeed } from "./agent-presets";
import type { CodingAgentDefinition } from "./types";

// #769 — FALLBACK_CODING_AGENTS is a second copy of the backend's embedded
// default (`src-tauri/resources/coding-agents/agents.default.json`). This is the
// FE half of the drift guard: it pins the fallback to the exact same 8 built-ins
// the backend ships, so the two copies cannot silently diverge (the backend's
// `embedded_default_matches_current_presets_exactly` pins the other half).
// #1861 - the eighth row is the beta Muse Code preset landed by #1860.
const EXPECTED_BUILTINS: Array<
  Pick<
    CodingAgentDefinition,
    "key" | "label" | "description" | "color" | "command" | "instructionsFilename"
  >
> = [
  { key: "claude", label: "Claude Code", description: "Coding Agent by Anthropic", color: "#d97706", command: "claude", instructionsFilename: "CLAUDE.md" },
  { key: "codex", label: "Codex", description: "Coding Agent by OpenAI", color: "#10b981", command: "codex", instructionsFilename: "AGENTS.md" },
  { key: "hermes", label: "Hermes", description: "Coding Agent by Nous Research", color: "#8b5cf6", command: "hermes", instructionsFilename: "AGENTS.md" },
  { key: "cursor", label: "Cursor CLI", description: "Coding Agent by Cursor", color: "#22d3ee", command: "agent", instructionsFilename: "AGENTS.md" },
  { key: "pi", label: "Pi", description: "Coding Agent by Earendil Inc", color: "#ec4899", command: "pi", instructionsFilename: "AGENTS.md" },
  { key: "opencode", label: "OpenCode", description: "Open-source terminal coding agent by Anomaly", color: "#64748b", command: "opencode", instructionsFilename: "AGENTS.md" },
  { key: "antigravity", label: "Antigravity", description: "Coding Agent by Google", color: "#4285F4", command: "agy", instructionsFilename: "AGENTS.md" },
  { key: "muse", label: "Muse Code", description: "Meta terminal coding agent (beta; macOS/Linux host only)", color: "#0668E1", command: "muse" },
];

/** #1861 - the exact shipped Muse row; instructionsFilename and configSeed are deliberately absent. */
const EXPECTED_MUSE: CodingAgentDefinition = {
  key: "muse",
  label: "Muse Code",
  description: "Meta terminal coding agent (beta; macOS/Linux host only)",
  color: "#0668E1",
  command: "muse",
  envs: [],
  isolatedHome: false,
  removable: true,
  updateCommands: [],
  autoUpdate: false,
};

describe("FALLBACK_CODING_AGENTS drift guard (#769)", () => {
  it("matches the backend embedded default: 8 built-ins, exact order and fields", () => {
    expect(FALLBACK_CODING_AGENTS).toHaveLength(8);
    expect(FALLBACK_CODING_AGENTS.map((a) => a.key)).toEqual([
      "claude",
      "codex",
      "hermes",
      "cursor",
      "pi",
      "opencode",
      "antigravity",
      "muse",
    ]);
    for (const expected of EXPECTED_BUILTINS) {
      const actual = FALLBACK_CODING_AGENTS.find((a) => a.key === expected.key);
      expect(actual).toBeTruthy();
      expect(actual).toMatchObject(expected);
    }
  });

  it("preserves the #766/#768 catalog facts: no Gemini, Cursor CLI runs `agent`, OpenCode 'by Anomaly'", () => {
    expect(FALLBACK_CODING_AGENTS.some((a) => a.key === "gemini")).toBe(false);
    expect(FALLBACK_CODING_AGENTS.find((a) => a.key === "cursor")?.command).toBe("agent");
    expect(FALLBACK_CODING_AGENTS.find((a) => a.key === "opencode")?.description).toBe(
      "Open-source terminal coding agent by Anomaly",
    );
  });

  it("#1861: the muse row is the eighth and last entry and mirrors the backend beta preset exactly", () => {
    const muse = FALLBACK_CODING_AGENTS[7];
    expect(muse).toStrictEqual(EXPECTED_MUSE);
    expect(muse).toBe(FALLBACK_CODING_AGENTS.find((a) => a.key === "muse"));
    // Optional catalog fields must be absent, not merely undefined.
    expect(muse).not.toHaveProperty("instructionsFilename");
    expect(muse).not.toHaveProperty("configSeed");
    expect(Object.keys(muse)).toEqual([
      "key",
      "label",
      "description",
      "color",
      "command",
      "envs",
      "isolatedHome",
      "removable",
      "updateCommands",
      "autoUpdate",
    ]);
  });

  it("ships every built-in removable with no Phase-1 config seed", () => {
    for (const def of FALLBACK_CODING_AGENTS) {
      expect(def.removable).toBe(true);
      expect(def.envs).toEqual([]);
      expect(def.isolatedHome).toBe(false);
      expect(def.configSeed).toBeUndefined();
    }
  });

  it("#1318/#1325/#1546/#1861: claude, pi, codex, hermes, opencode, and antigravity ship update commands; cursor and muse ship none; every entry defaults autoUpdate off", () => {
    // The six verified update commands stay exact and Cursor/Muse are the only
    // empty-update defaults, so Muse never enters the managed-update rows.
    expect(
      FALLBACK_CODING_AGENTS.filter((def) => def.updateCommands.length > 0).map((def) => [def.key, def.updateCommands]),
    ).toEqual([
      ["claude", ["claude --update"]],
      ["codex", ["codex update"]],
      ["hermes", ["hermes update --yes"]],
      ["pi", ["pi update"]],
      ["opencode", ["opencode upgrade"]],
      ["antigravity", ["agy update"]],
    ]);
    expect(FALLBACK_CODING_AGENTS.filter((def) => def.updateCommands.length === 0).map((def) => def.key)).toEqual([
      "cursor",
      "muse",
    ]);
    for (const def of FALLBACK_CODING_AGENTS) {
      expect(def.autoUpdate).toBe(false);
      if (def.key === "claude") {
        expect(def.updateCommands).toEqual(["claude --update"]);
      } else if (def.key === "pi") {
        expect(def.updateCommands).toEqual(["pi update"]);
      } else if (def.key === "codex") {
        expect(def.updateCommands).toEqual(["codex update"]);
      } else if (def.key === "hermes") {
        expect(def.updateCommands).toEqual(["hermes update --yes"]);
      } else if (def.key === "opencode") {
        expect(def.updateCommands).toEqual(["opencode upgrade"]);
      } else if (def.key === "antigravity") {
        expect(def.updateCommands).toEqual(["agy update"]);
      } else {
        expect(def.updateCommands).toEqual([]);
      }
    }
  });
});

describe("definitionToSeed (#769)", () => {
  it("strips catalog-only fields and keeps the AgentConfig seed", () => {
    const claude = FALLBACK_CODING_AGENTS.find((a) => a.key === "claude")!;
    const seed = definitionToSeed(claude);
    expect(seed).toEqual({
      label: "Claude Code",
      command: "claude",
      color: "#d97706",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "CLAUDE.md",
    });
    // Catalog-only fields must not leak into the persisted agent.
    expect("key" in seed).toBe(false);
    expect("description" in seed).toBe(false);
    expect("removable" in seed).toBe(false);
  });

  it("omits instructionsFilename and configSeed when the definition lacks them", () => {
    const bare: CodingAgentDefinition = {
      key: "bare",
      label: "Bare",
      description: "no extras",
      color: "#000000",
      command: "bare",
      envs: [],
      isolatedHome: false,
      removable: true,
      updateCommands: [],
      autoUpdate: false,
    };
    const seed = definitionToSeed(bare);
    expect("instructionsFilename" in seed).toBe(false);
    expect("configSeed" in seed).toBe(false);
    // #1318: the update fields are catalog-only and must never leak into the
    // persisted agent.
    expect("updateCommands" in seed).toBe(false);
    expect("autoUpdate" in seed).toBe(false);
  });

  it("#1861: the muse row seeds only the AgentConfig core; instructionsFilename and configSeed stay absent", () => {
    const muse = FALLBACK_CODING_AGENTS.find((a) => a.key === "muse")!;
    expect(muse).toBeTruthy();
    const seed = definitionToSeed(muse);
    expect(seed).toStrictEqual({
      label: "Muse Code",
      command: "muse",
      color: "#0668E1",
      envs: [],
      isolatedHome: false,
    });
    expect(seed).not.toHaveProperty("instructionsFilename");
    expect(seed).not.toHaveProperty("configSeed");
    expect("updateCommands" in seed).toBe(false);
    expect("autoUpdate" in seed).toBe(false);
  });

  it("#1482: the antigravity row seeds the agy command with AGENTS.md", () => {
    const antigravity = FALLBACK_CODING_AGENTS.find((a) => a.key === "antigravity")!;
    expect(antigravity).toBeTruthy();
    expect(definitionToSeed(antigravity)).toMatchObject({
      command: "agy",
      instructionsFilename: "AGENTS.md",
    });
  });
});
