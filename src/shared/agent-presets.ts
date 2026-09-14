import type { AgentConfig, CodingAgentDefinition } from "./types";

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

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
