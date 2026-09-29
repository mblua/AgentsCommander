import { describe, expect, it } from "vitest";
import {
  FALLBACK_CODING_AGENTS,
  compareWelcomeAgents,
  definitionToSeed,
  sortWelcomeAgents,
  welcomeVendorOf,
  WELCOME_PINNED_KEYS,
} from "./agent-presets";
import type { CodingAgentDefinition, CodingAgentWelcomeStatus } from "./types";

// #769 — second copy of the backend's ENABLED built-ins
// (`src-tauri/resources/coding-agents/agents.default.json`). This is the FE half
// of the drift guard: it pins the enabled set and order, so the two copies cannot
// silently diverge (the backend's `embedded_default_matches_current_presets_exactly`
// pins the 9-row embedded default).
// #1912 — the mirror rule: FALLBACK_CODING_AGENTS must equal the ENABLED rows of
// `BUILTIN_AGENT_SUPPORT` (`src-tauri/src/config/coding_agents_catalog.rs`), in
// the same order; it must never resurrect a de-supported built-in.
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
  { key: "grok", label: "Grok Build", description: "Coding agent Grok Build", color: "#64748b", command: "grok", instructionsFilename: "AGENTS.md" },
];

describe("FALLBACK_CODING_AGENTS drift guard (#769)", () => {
  it("matches the enabled built-ins: 8 rows (muse disabled, grok added), exact order and fields", () => {
    expect(FALLBACK_CODING_AGENTS.map((a) => a.key)).toEqual([
      "claude",
      "codex",
      "hermes",
      "cursor",
      "pi",
      "opencode",
      "antigravity",
      "grok",
    ]);
    // #1999 — muse stays in the embedded default as a disabled row, so the
    // enabled-row mirror must not carry it.
    expect(FALLBACK_CODING_AGENTS.some((a) => a.key === "muse")).toBe(false);
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

  it("#1318/#1325/#1546: claude, pi, codex, hermes, opencode, and antigravity ship update commands; cursor and grok ship none; every entry defaults autoUpdate off", () => {
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
    // #2736: installCommands is catalog-only too.
    const withInstall = definitionToSeed({ ...bare, installCommands: { default: "npm i -g bare" } });
    expect("installCommands" in withInstall).toBe(false);
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

function welcomeDef(key: string): CodingAgentDefinition {
  return {
    key,
    label: key,
    description: key,
    color: "#000000",
    command: key,
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
  };
}

function row(
  key: string,
  installed: boolean,
  testedLevel: CodingAgentWelcomeStatus["testedLevel"],
): CodingAgentWelcomeStatus {
  return { key, installed, testedLevel, installCommand: null };
}

function keysOf(defs: CodingAgentDefinition[]): string[] {
  return defs.map((def) => def.key);
}

describe("Welcome order (#2736)", () => {
  it("compareWelcomeAgents_2736_puts_installed_first", () => {
    const lowInstalled = welcomeDef("low-installed");
    const highMissing = welcomeDef("high-missing");
    const status = new Map([
      ["low-installed", row("low-installed", true, "low")],
      ["high-missing", row("high-missing", false, "high")],
    ]);
    const index = new Map([["high-missing", 0], ["low-installed", 1]]);
    expect(compareWelcomeAgents(lowInstalled, highMissing, status, index)).toBeLessThan(0);
    expect(compareWelcomeAgents(highMissing, lowInstalled, status, index)).toBeGreaterThan(0);
  });

  it("compareWelcomeAgents_2736_orders_high_medium_low_then_no_level", () => {
    const catalog = ["none", "low", "medium", "high"].map(welcomeDef);
    const status = [
      row("none", false, null),
      row("low", false, "low"),
      row("medium", false, "medium"),
      row("high", false, "high"),
    ];
    expect(keysOf(sortWelcomeAgents(catalog, status))).toEqual(["high", "medium", "low", "none"]);
  });

  it("compareWelcomeAgents_2736_uses_catalog_order_as_the_tie_break", () => {
    const catalog = ["second", "first"].map(welcomeDef);
    const status = [row("first", false, "high"), row("second", false, "high")];
    expect(keysOf(sortWelcomeAgents(catalog, status))).toEqual(["second", "first"]);
    const statusMap = new Map(status.map((r) => [r.key, r]));
    const index = new Map([["second", 0], ["first", 1]]);
    expect(compareWelcomeAgents(catalog[0], catalog[1], statusMap, index)).toBeLessThan(0);
  });

  it("sortWelcomeAgents_2736_does_not_mutate_its_input", () => {
    const catalog = ["b", "a"].map(welcomeDef);
    const snapshot = [...catalog];
    const result = sortWelcomeAgents(catalog, [row("a", true, "high")]);
    expect(keysOf(result)).toEqual(["a", "b"]);
    expect(result).not.toBe(catalog);
    expect(catalog).toEqual(snapshot);
    expect(catalog[0]).toBe(snapshot[0]);
    expect(catalog[1]).toBe(snapshot[1]);
  });

  it("sortWelcomeAgents_2736_keeps_a_key_with_no_status_row_after_low", () => {
    const catalog = ["unknown", "low"].map(welcomeDef);
    // "ghost" is a status row with no catalog entry: ignored.
    const status = [row("low", false, "low"), row("ghost", true, "high")];
    expect(keysOf(sortWelcomeAgents(catalog, status))).toEqual(["low", "unknown"]);
  });

  it("welcomeVendorOf_2784_reads_the_text_after_the_last_by", () => {
    expect(welcomeVendorOf("Coding Agent by Anthropic")).toBe("Anthropic");
    expect(welcomeVendorOf("Coding Agent by OpenAI")).toBe("OpenAI");
    expect(welcomeVendorOf("Coding Agent by Nous Research")).toBe("Nous Research");
    expect(welcomeVendorOf("Coding Agent by Cursor")).toBe("Cursor");
    expect(welcomeVendorOf("Coding Agent by Earendil Inc")).toBe("Earendil Inc");
    expect(welcomeVendorOf("Open-source terminal coding agent by Anomaly")).toBe("Anomaly");
    expect(welcomeVendorOf("Coding Agent by Google")).toBe("Google");
    expect(welcomeVendorOf("Coding agent Grok Build")).toBeNull();
    expect(welcomeVendorOf("Configure your own Coding Agent")).toBeNull();
  });

  it("sortWelcomeAgents_2784_puts_opencode_first_among_the_low_agents", () => {
    const catalog = ["hermes", "cursor", "opencode", "grok"].map(welcomeDef);
    const status = catalog.map((def) => row(def.key, false, "low"));
    expect(keysOf(sortWelcomeAgents(catalog, status))).toEqual([
      "opencode",
      "hermes",
      "cursor",
      "grok",
    ]);
  });

  it("sortWelcomeAgents_2784_never_lifts_opencode_above_a_better_tested_or_installed_agent", () => {
    const catalog = ["opencode", "claude"].map(welcomeDef);
    expect(
      keysOf(sortWelcomeAgents(catalog, [row("claude", false, "high"), row("opencode", false, "low")])),
    ).toEqual(["claude", "opencode"]);
    expect(
      keysOf(sortWelcomeAgents(catalog, [row("claude", true, "low"), row("opencode", false, "low")])),
    ).toEqual(["claude", "opencode"]);
  });

  it("WELCOME_PINNED_KEYS_2784_is_exactly_opencode", () => {
    expect(WELCOME_PINNED_KEYS).toEqual(["opencode"]);
  });
});
