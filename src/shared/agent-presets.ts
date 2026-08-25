import type { AgentConfig, CodingAgentDefinition } from "./types";

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
    // #1318/#1325/#1546 - mirror of the embedded default: claude, pi, codex,
    // hermes, opencode, and antigravity ship the update command; cursor ships
    // none; every entry defaults autoUpdate to false.
    updateCommands: ["claude --update"],
    autoUpdate: false,
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
    updateCommands: ["codex update"],
    autoUpdate: false,
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
    updateCommands: ["hermes update --yes"],
    autoUpdate: false,
  },
  {
    key: "cursor",
    label: "Cursor CLI",
    description: "Coding Agent by Cursor",
    color: "#22d3ee",
    command: "agent",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
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
    updateCommands: ["pi update"],
    autoUpdate: false,
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
    updateCommands: ["opencode upgrade"],
    autoUpdate: false,
  },
  {
    key: "antigravity",
    label: "Antigravity",
    description: "Coding Agent by Google",
    color: "#4285F4",
    command: "agy",
    instructionsFilename: "AGENTS.md",
    envs: [],
    isolatedHome: false,
    removable: true,
    // #1482/#1546 - mirror of the embedded default: Antigravity ships the verified 'agy update' command (autoUpdate stays false).
    updateCommands: ["agy update"],
    autoUpdate: false,
  },
];

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
