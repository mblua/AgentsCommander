import type { AgentConfig, CodingAgentDefinition } from "./types";

/**
 * #769 — the coding-agent catalog is now backend-owned (the seeded, user-editable
 * `<config_dir>/coding-agents/agents.json`, exposed by `get_coding_agent_catalog`
 * and consumed through `codingAgentsStore`). This module keeps only:
 *
 *  - `newAgentId()` — id minter for the "+ Add" flow (unchanged), and
 *  - `FALLBACK_CODING_AGENTS` — a synchronous never-empty seed and the offline
 *    fallback the store falls back to when the IPC transport itself fails.
 *
 * `FALLBACK_CODING_AGENTS` is a second copy of the backend's embedded default
 * (`src-tauri/resources/coding-agents/agents.default.json`). The duplication is
 * intentional (it is what makes the onboarding/settings list resilient to a dead
 * backend), and a drift-guard test keeps the two copies in lockstep with the
 * backend's `embedded_default_matches_current_presets_exactly` test.
 */
export const FALLBACK_CODING_AGENTS: CodingAgentDefinition[] = [
  {
    key: "claude",
    label: "Claude Code",
    description: "Coding Agent by Anthropic",
    color: "#d97706",
    command: "claude",
    instructionsFilename: "CLAUDE.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
  {
    key: "codex",
    label: "Codex",
    description: "Coding Agent by OpenAI",
    color: "#10b981",
    command: "codex",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
  {
    key: "hermes",
    label: "Hermes",
    description: "Coding Agent by Nous Research",
    color: "#8b5cf6",
    command: "hermes",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
  {
    key: "cursor",
    label: "Cursor CLI",
    description: "Coding Agent by Cursor",
    color: "#22d3ee",
    // Cursor's CLI executable is `agent`, not `cursor`.
    command: "agent",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
  {
    key: "pi",
    label: "Pi",
    description: "Coding Agent by Earendil Inc",
    color: "#ec4899",
    command: "pi",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
  {
    key: "opencode",
    label: "OpenCode",
    description: "Open-source terminal coding agent by Anomaly",
    color: "#64748b",
    command: "opencode",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
  },
];

/**
 * Project a catalog definition onto the persisted agent seed the onboarding and
 * settings "+ Add" flows feed to `addAgent` — i.e. `Omit<AgentConfig, "id">`,
 * dropping the catalog-only `{ key, description, removable }`. `instructionsFilename`
 * and `configSeed` are carried through only when present, matching how the old
 * `AGENT_PRESET_MAP.<key>` values were shaped.
 */
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

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
