import type { AgentConfig } from "../../../shared/types";

/** Labels of the other rows whose saved `id_hash` equals row `index`'s, in row order. */
export function sharedIdentityPeers(agents: readonly AgentConfig[], index: number): string[] {
  const hash = agents[index]?.id_hash;
  if (!hash) return [];
  return agents.filter((a, i) => i !== index && a.id_hash === hash).map((a) => a.label);
}
