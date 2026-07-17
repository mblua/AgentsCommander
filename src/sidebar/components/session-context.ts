import type { AgentConfig } from "../../shared/types";

export function contextBadgeConfigured(
  agents: AgentConfig[] | undefined,
  agentId: string | null | undefined,
): boolean {
  if (!agentId) return false;
  return !!agents?.find((a) => a.id === agentId)?.contextRegex?.trim();
}

export function contextBadgeText(percent: number | null | undefined): string {
  if (percent === null || percent === undefined) return "CTX N/A";
  return `CTX ${percent}%`;
}
