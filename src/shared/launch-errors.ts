// Maps backend `SessionAPI.create` rejections into user-facing copy.
//
// The Resource Monitor concurrency cap (src-tauri `registry.rs`
// `try_reserve_agent_slot`) rejects a launch *before* any session is created
// with the raw string:
//   "Resource Monitor cap reached: {active}/{max} agent groups are active"
// That is turned into a short, actionable message. Every other failure is
// surfaced verbatim so nothing is silently swallowed (#516).

const CAP_PREFIX = "Resource Monitor cap reached";

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
  return raw || "Failed to start agent.";
}
