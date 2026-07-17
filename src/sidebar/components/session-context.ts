import type { AgentConfig } from "../../shared/types";

/**
 * #1033 - no regex configured for this session's agent => no badge at all. Not N/A,
 * not a chip, no visual noise. One resolver for all three surfaces, mirroring #548's
 * rule for the profile tooltip: no second resolver, or the three drift.
 *
 * Takes its inputs as arguments rather than reading a session, because the three
 * surfaces reach their session three different ways (`props.session` in SessionItem,
 * `rootSession()` in RootAgentBanner, `session()` in ProjectPanel); a gate written
 * against one of them compiles in exactly one of them.
 *
 * The `.trim()` here is a VISIBILITY test only and never touches a stored or
 * transmitted value - the stored pattern is byte-for-byte what the user typed, and
 * its leading whitespace is load-bearing. This exists so a legacy file hand-edited
 * to `"contextRegex": "   "` does not show a permanent `N/A`.
 *
 * Keys by agent id and never by `command`: two agents may share `"command": "claude"`
 * while only one configures a pattern.
 */
export function contextBadgeConfigured(
  agents: AgentConfig[] | undefined,
  agentId: string | null | undefined,
): boolean {
  if (!agentId) return false;
  return !!agents?.find((a) => a.id === agentId)?.contextRegex?.trim();
}

/**
 * #1033 Presentation projection: a context reading -> badge text. Total.
 * Editing a value here changes pixels only; no behavior reads this string.
 *
 * Deliberately NOT injective: `null` (the engine says unavailable) and `undefined`
 * (no event and no snapshot has spoken for this session yet) both map to `CTX N/A`,
 * because unavailable is exactly one thing.
 *
 * The `=== null || === undefined` test is deliberate and must not be "simplified" to
 * a truthiness check: `0` is a REAL reading and renders `CTX 0%`. A `percent ? ... :`
 * here turns a true zero into `N/A`, keeps every other case working, and is invisible
 * on screen.
 */
export function contextBadgeText(percent: number | null | undefined): string {
  if (percent === null || percent === undefined) return "CTX N/A";
  return `CTX ${percent}%`;
}
