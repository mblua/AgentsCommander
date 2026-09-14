import type { AgentConfig, CodingAgentDefinition } from "./types";

// Mirror of the ENABLED rows of `BUILTIN_AGENT_SUPPORT`
// (`src-tauri/src/config/coding_agents_catalog.rs`), in `agents.default.json`
// order. `agents.default.json` keeps muse's disabled row; this mirror carries
// enabled rows only, so it must never resurrect a de-supported built-in.
// #1965 — not served on an IPC failure anymore: the catalog store disables
// registrations instead (`src/sidebar/stores/coding-agents.ts`), so this is the
// drift-guard mirror only.
export const FALLBACK_CODING_AGENTS: CodingAgentDefinition[] = [
  {
    key: "claude",
    label: "Claude Code",
    description: "Coding Agent by Anthropic",
    color: "#d97706",
    command: "claude",
    instructionsFilename: "CLAUDE.md",
    // #1318/#1325/#1546 - mirror of the embedded default: claude, pi, codex,
    // hermes, opencode, and antigravity ship the update command; cursor and
    // grok ship none; every entry defaults autoUpdate to false.
    updateCommands: ["claude --update"],
  },
  {
    key: "codex",
    label: "Codex",
    description: "Coding Agent by OpenAI",
    color: "#10b981",
    command: "codex",
    instructionsFilename: "AGENTS.md",
    updateCommands: ["codex update"],
  },
  {
    key: "hermes",
    label: "Hermes",
    description: "Coding Agent by Nous Research",
    color: "#8b5cf6",
    command: "hermes",
    instructionsFilename: "AGENTS.md",
    updateCommands: ["hermes update --yes"],
  },
  {
    key: "cursor",
    label: "Cursor CLI",
    description: "Coding Agent by Cursor",
    color: "#22d3ee",
    command: "agent",
    instructionsFilename: "AGENTS.md",
    updateCommands: [],
  },
  {
    key: "pi",
    label: "Pi",
    description: "Coding Agent by Earendil Inc",
    color: "#ec4899",
    command: "pi",
    instructionsFilename: "AGENTS.md",
    updateCommands: ["pi update"],
  },
  {
    key: "opencode",
    label: "OpenCode",
    description: "Open-source terminal coding agent by Anomaly",
    color: "#64748b",
    command: "opencode",
    instructionsFilename: "AGENTS.md",
    updateCommands: ["opencode upgrade"],
  },
  {
    key: "antigravity",
    label: "Antigravity",
    description: "Coding Agent by Google",
    color: "#4285F4",
    command: "agy",
    instructionsFilename: "AGENTS.md",
    // #1482/#1546 - mirror of the embedded default: Antigravity ships the verified 'agy update' command (autoUpdate stays false).
    updateCommands: ["agy update"],
  },
  {
    key: "grok",
    label: "Grok Build",
    description: "Coding agent Grok Build",
    color: "#64748b",
    command: "grok",
    instructionsFilename: "AGENTS.md",
    updateCommands: [],
  },
].map((definition): CodingAgentDefinition => ({
  ...definition,
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

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
