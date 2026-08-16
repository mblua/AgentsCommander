import { describe, expect, it } from "vitest";
import { FALLBACK_CODING_AGENTS, definitionToSeed } from "./agent-presets";
import type { CodingAgentDefinition } from "./types";

// #769 — FALLBACK_CODING_AGENTS is a second copy of the backend's embedded
// default (`src-tauri/resources/coding-agents/agents.default.json`). This is the
// FE half of the drift guard: it pins the fallback to the exact same 6 built-ins
// the backend ships, so the two copies cannot silently diverge (the backend's
// `embedded_default_matches_current_presets_exactly` pins the other half).
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
];

describe("FALLBACK_CODING_AGENTS drift guard (#769)", () => {
  it("matches the backend embedded default: 6 built-ins, exact order and fields", () => {
    expect(FALLBACK_CODING_AGENTS.map((a) => a.key)).toEqual([
      "claude",
      "codex",
      "hermes",
      "cursor",
      "pi",
      "opencode",
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

  it("ships every built-in removable with no Phase-1 config seed", () => {
    for (const def of FALLBACK_CODING_AGENTS) {
      expect(def.removable).toBe(true);
      expect(def.envs).toEqual([]);
      expect(def.isolatedHome).toBe(false);
      expect(def.configSeed).toBeUndefined();
    }
  });

  it("#1318/#1325: claude, pi, and codex ship update commands; every entry defaults autoUpdate off", () => {
    for (const def of FALLBACK_CODING_AGENTS) {
      expect(def.autoUpdate).toBe(false);
      if (def.key === "claude") {
        expect(def.updateCommands).toEqual(["claude --update"]);
      } else if (def.key === "pi") {
        expect(def.updateCommands).toEqual(["pi update"]);
      } else if (def.key === "codex") {
        expect(def.updateCommands).toEqual(["codex update"]);
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
});
