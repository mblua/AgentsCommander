// Maps backend `SessionAPI.create` rejections into user-facing copy.
//
// The Resource Monitor concurrency cap (src-tauri `registry.rs`
// `try_reserve_agent_slot`) rejects a launch *before* any session is created
// with the raw string:
//   "Resource Monitor cap reached: {active}/{max} agent groups are active"
// That is turned into a short, actionable message. Every other failure is
// surfaced verbatim so nothing is silently swallowed (#516).

const CAP_PREFIX = "Resource Monitor cap reached";

// A restart whose stored coding-agent id no longer exists (src-tauri
// `session.rs`, #2434) rejects with
//   "unresolved_coding_agent_reference: '<id>' matches no configured coding agent"
// That is turned into plain words that keep the vanished id (#2573).
const UNRESOLVED_AGENT_RE = /^unresolved_coding_agent_reference:\s*'([^']*)'/;
const UNRESOLVED_AGENT_PREFIX = "unresolved_coding_agent_reference";
const UNRESOLVED_AGENT_FIX =
  "Right-click the session, choose Coding Agent, pick one, then restart.";

/**
 * User-facing message for a launch failure. The cap rejection becomes a
 * friendly, actionable string (preserving the active/max counts when present);
 * anything else is passed through unchanged.
 */
export function launchErrorMessage(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? "");
  if (raw.startsWith(CAP_PREFIX)) {
    const counts = raw.match(/(\d+)\s*\/\s*(\d+)/);
    const ratio = counts ? `${counts[1]}/${counts[2]}` : null;
    const head = ratio
      ? `Resource Monitor cap reached (${ratio}).`
      : "Resource Monitor cap reached.";
    return `${head} Close an agent or raise the limit in Settings > Resources.`;
  }
  if (raw.startsWith(UNRESOLVED_AGENT_PREFIX)) {
    const id = raw.match(UNRESOLVED_AGENT_RE)?.[1];
    const agent = id ? `its coding agent '${id}'` : "its coding agent";
    return `Can't restart this session: ${agent} is no longer in Settings. ${UNRESOLVED_AGENT_FIX}`;
  }
  return raw || "Failed to start agent.";
}
