import type { Session } from "../../shared/types";

export type RootAgentAction =
  | { kind: "create"; agentId: string }
  | { kind: "restart"; id: string; agentId: string; skipAutoResume?: boolean };

/**
 * Decide how to apply a coding-agent selection from the Root Agent banner.
 *
 * Live root → fresh restart (skipAutoResume defaults true on the backend).
 * Dormant root (status is an `{ exited }` object) → restart with
 * skipAutoResume:false so the provider's resume flag is forwarded
 * (`claude --continue`, `codex resume --last`, `antigravity --continue`,
 * `muse resume --last` when eligible; #1873).
 *
 * #1861 - this mapping is provider-neutral: the frontend sends only the
 * fresh/resume intent and the backend owns every provider's effective argv,
 * Muse included. No provider branch belongs here.
 */
export function rootAgentCodingAgentAction(
  root: Session | undefined,
  agentId: string,
): RootAgentAction {
  if (!root) return { kind: "create", agentId };
  const dormant = typeof root.status !== "string";
  if (dormant) return { kind: "restart", id: root.id, agentId, skipAutoResume: false };
  return { kind: "restart", id: root.id, agentId };
}
