import type { AgentConfig } from "./types";

export interface AgentPreset {
  key: string;
  label: string;
  description: string;
  color: string;
  config: Omit<AgentConfig, "id">;
}

export const AGENT_PRESETS: AgentPreset[] = [
  {
    key: "claude",
    label: "Claude Code",
    description: "Coding Agent by Anthropic",
    color: "#d97706",
    config: {
      label: "Claude Code",
      command: "claude",
      color: "#d97706",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "CLAUDE.md",
    },
  },
  {
    key: "codex",
    label: "Codex",
    description: "Coding Agent by OpenAI",
    color: "#10b981",
    config: {
      label: "Codex",
      command: "codex",
      color: "#10b981",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "AGENTS.md",
    },
  },
  {
    key: "hermes",
    label: "Hermes",
    description: "Coding Agent by Nous Research",
    color: "#8b5cf6",
    config: {
      label: "Hermes",
      command: "hermes",
      color: "#8b5cf6",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "AGENTS.md",
    },
  },
  {
    key: "cursor",
    label: "Cursor CLI",
    description: "Coding Agent by Cursor",
    color: "#22d3ee",
    config: {
      label: "Cursor CLI",
      // Cursor's CLI executable is `agent`, not `cursor`.
      command: "agent",
      color: "#22d3ee",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "AGENTS.md",
    },
  },
  {
    key: "pi",
    label: "Pi",
    description: "Coding Agent by Earendil Inc",
    color: "#ec4899",
    config: {
      label: "Pi",
      command: "pi",
      color: "#ec4899",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "AGENTS.md",
    },
  },
  {
    key: "opencode",
    label: "OpenCode",
    description: "Open-source terminal coding agent by SST",
    color: "#64748b",
    config: {
      label: "OpenCode",
      command: "opencode",
      color: "#64748b",
      envs: [],
      isolatedHome: false,
      instructionsFilename: "AGENTS.md",
    },
  },
];

/** Record-based lookup for SettingsModal quick-add buttons */
export const AGENT_PRESET_MAP: Record<string, Omit<AgentConfig, "id">> =
  Object.fromEntries(AGENT_PRESETS.map((p) => [p.key, p.config]));

let idCounter = 0;
export function newAgentId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}
