import type {
  AgentConfig,
  CodingAgentDefinition,
  CodingAgentTestedLevel,
  CodingAgentWelcomeStatus,
} from "./types";

// Mirror of the ENABLED rows of `BUILTIN_AGENT_SUPPORT`
// (`src-tauri/src/config/coding_agents_catalog.rs`), in `agents.default.json`
// order. `agents.default.json` keeps muse's disabled row; this mirror carries
// enabled rows only, so it must never resurrect a de-supported built-in.
// #1965 — not served on an IPC failure anymore: the catalog store disables
// registrations instead (`src/sidebar/stores/coding-agents.ts`), so this is the
// drift-guard mirror only.
export const FALLBACK_CODING_AGENTS: CodingAgentDefinition[] = ([
  // #1318/#1325/#1546 - mirror of the embedded default: claude, pi, codex,
  // hermes, opencode, and antigravity ship the update command; cursor and
  // grok ship none; every entry defaults autoUpdate to false.
  ["claude", "Claude Code", "Coding Agent by Anthropic", "#d97706", "claude", "CLAUDE.md", ["claude --update"]],
  ["codex", "Codex", "Coding Agent by OpenAI", "#10b981", "codex", "AGENTS.md", ["codex update"]],
  ["hermes", "Hermes", "Coding Agent by Nous Research", "#8b5cf6", "hermes", "AGENTS.md", ["hermes update --yes"]],
  ["cursor", "Cursor CLI", "Coding Agent by Cursor", "#22d3ee", "agent", "AGENTS.md", []],
  ["pi", "Pi", "Coding Agent by Earendil Inc", "#ec4899", "pi", "AGENTS.md", ["pi update"]],
  ["opencode", "OpenCode", "Open-source terminal coding agent by Anomaly", "#64748b", "opencode", "AGENTS.md", ["opencode upgrade"]],
  // #1482/#1546 - mirror of the embedded default: Antigravity ships the verified 'agy update' command (autoUpdate stays false).
  ["antigravity", "Antigravity", "Coding Agent by Google", "#4285F4", "agy", "AGENTS.md", ["agy update"]],
  ["grok", "Grok Build", "Coding agent Grok Build", "#64748b", "grok", "AGENTS.md", []],
] satisfies Array<[
  key: string, label: string, description: string, color: string,
  command: string, instructionsFilename: string, updateCommands: string[],
]>).map(([key, label, description, color, command,
          instructionsFilename, updateCommands]): CodingAgentDefinition => ({
  key, label, description, color, command, instructionsFilename, updateCommands,
  envs: [],
  isolatedHome: false,
  removable: true,
  autoUpdate: false,
}));

export function definitionToSeed(def: CodingAgentDefinition): Omit<AgentConfig, "id"> {
  const seed: Omit<AgentConfig, "id"> = {
    label: def.label,
    command: def.command,
    color: def.color,
    envs: def.envs,
    isolatedHome: def.isolatedHome,
  };
  if (def.instructionsFilename !== undefined) {
    seed.instructionsFilename = def.instructionsFilename;
  }
  if (def.configSeed !== undefined) {
    seed.configSeed = def.configSeed;
  }
  return seed;
}

/** #2784 - Welcome cards show "by <Vendor>" instead of the catalog description.
 *  The vendor is the text after the LAST "by " of the description; a description
 *  with no "by " has no vendor and the caller renders it unchanged. */
export function welcomeVendorOf(description: string): string | null {
  const m = /^.*\bby\s+(\S.*)$/i.exec(description.trim());
  return m ? m[1].trim() : null;
}

/** #2784 R3 - CODE ONLY vendor overrides, keyed by catalog key. Never read from,
 *  and never patchable through, any catalog file: `agents.json` is seeded once and
 *  never rewritten (`coding_agents_catalog.rs:23-27`), so a catalog description edit
 *  would reach new installs only. Same pattern as `BUILTIN_TESTED_LEVEL`.
 *  One entry per key whose description carries no "by <Vendor>". */
export const WELCOME_VENDOR_BY_KEY: Readonly<Record<string, string | undefined>> = { grok: "SpaceXAI" };

/** #2784 R3 - vendor for a Welcome card: the override table first, then the
 *  description derivation, then null (the caller renders the description). */
export function welcomeVendorForKey(key: string, description: string): string | null {
  const override = WELCOME_VENDOR_BY_KEY[key];
  return typeof override === "string" ? override : welcomeVendorOf(description);
}

const TESTED_LEVEL_RANK: Record<CodingAgentTestedLevel, number> = { high: 0, medium: 1, low: 2 };
const NO_LEVEL_RANK = 3;
/** #2784 - keys pinned to the FRONT of their tested-level group, in this order.
 *  Requirement: OpenCode is first among the Tested: Low agents. */
export const WELCOME_PINNED_KEYS: readonly string[] = ["opencode"];
const pinnedRank = (key: string): number => {
  const index = WELCOME_PINNED_KEYS.indexOf(key);
  return index < 0 ? WELCOME_PINNED_KEYS.length : index;
};

/** #2736 - Welcome order: installed first, then High > Medium > Low > no level,
 *  then catalog order. "Custom Agent" is NOT passed here; the caller appends it
 *  last. A key with no status row is treated as not installed with no level. */
export function compareWelcomeAgents(
  a: CodingAgentDefinition,
  b: CodingAgentDefinition,
  status: Map<string, CodingAgentWelcomeStatus>,
  catalogIndex: Map<string, number>,
): number {
  const rowA = status.get(a.key);
  const rowB = status.get(b.key);
  const installedA = rowA?.installed ? 0 : 1;
  const installedB = rowB?.installed ? 0 : 1;
  if (installedA !== installedB) return installedA - installedB;
  const levelA = rowA?.testedLevel ? TESTED_LEVEL_RANK[rowA.testedLevel] : NO_LEVEL_RANK;
  const levelB = rowB?.testedLevel ? TESTED_LEVEL_RANK[rowB.testedLevel] : NO_LEVEL_RANK;
  if (levelA !== levelB) return levelA - levelB;
  const pinnedA = pinnedRank(a.key);
  const pinnedB = pinnedRank(b.key);
  if (pinnedA !== pinnedB) return pinnedA - pinnedB;
  return (catalogIndex.get(a.key) ?? 0) - (catalogIndex.get(b.key) ?? 0);
}

/** #2736 - returns a NEW array in Welcome order; never mutates `catalog`. */
export function sortWelcomeAgents(
  catalog: CodingAgentDefinition[],
  status: CodingAgentWelcomeStatus[],
): CodingAgentDefinition[] {
  const statusByKey = new Map(status.map((row) => [row.key, row]));
  const catalogIndex = new Map(catalog.map((def, index) => [def.key, index]));
  return [...catalog].sort((a, b) => compareWelcomeAgents(a, b, statusByKey, catalogIndex));
}

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
