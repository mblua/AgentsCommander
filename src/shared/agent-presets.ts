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
      gitPullBefore: false,
      excludeGlobalClaudeMd: true,
      envs: [],
      isolatedHome: false,
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
      gitPullBefore: false,
      excludeGlobalClaudeMd: false,
      envs: [],
      isolatedHome: false,
    },
  },
  {
    key: "gemini",
    label: "Gemini CLI",
    description: "Coding Agent by Google",
    color: "#4285f4",
    config: {
      label: "Gemini CLI",
      command: "gemini",
      color: "#4285f4",
      gitPullBefore: false,
      excludeGlobalClaudeMd: false,
      envs: [],
      isolatedHome: false,
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
