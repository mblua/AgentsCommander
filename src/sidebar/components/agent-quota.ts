/** #2482 - null when there is nothing to draw. Otherwise the two widths, in
 *  percent, that the chip's gradient uses: remaining on the left, used on the
 *  right.
 *
 *  Unknown is exactly one thing. Everything that is not an integer in 0..=100 is
 *  unknown: null, undefined, NaN, a non-finite number, a negative, >100 and a
 *  non-integer. It returns null and the caller then emits NO class and NO inline
 *  style, so the chip is byte-identical to today's. 0 and 100 are REAL readings
 *  and are NOT unknown. */
export function quotaFill(
  weeklyUsedPercent: number | null | undefined,
): { remaining: number; used: number } | null {
  const value = weeklyUsedPercent;
  if (!(Number.isInteger(value) && (value as number) >= 0 && (value as number) <= 100)) return null;
  const used = value as number;
  return { remaining: 100 - used, used };
}

/** The chip's tooltip while a reading exists. Best-effort wording, matching
 *  CONTEXT_BADGE_TOOLTIP's honesty about staleness. */
export const AGENT_QUOTA_TOOLTIP_PREFIX = "Weekly (7-day) quota";

export function agentQuotaTitle(used: number): string {
  return `${AGENT_QUOTA_TOOLTIP_PREFIX}: ${100 - used}% remaining, ${used}% used`;
}

/** #2482 - every attribute the filled chip needs, from a label and a reading.
 *  Used by BOTH chips: `SessionItem.tsx` (p6) and the replica row (p7). With
 *  no reading it returns ONLY `class`, so a spread gives today's chip byte for
 *  byte, and the label stays the accessible name because nothing overrides it.
 */
export function quotaChipAttrs(
  agentLabel: string,
  weeklyUsedPercent: number | null | undefined,
): Record<string, unknown> {
  const fill = quotaFill(weeklyUsedPercent);
  if (!fill) return { class: "ac-discovery-badge agent" };
  const used = weeklyUsedPercent as number;
  return {
    class: "ac-discovery-badge agent quota-fill",
    style: { "--ac-quota-remaining": `${fill.remaining}%` },
    title: agentQuotaTitle(used),
    role: "meter",
    // Keeps the agent label: `aria-label` REPLACES the element's text, so a bare
    // "Weekly quota N% used" deletes "Claude Code" from every screen reader.
    "aria-label": `${agentLabel}, weekly quota ${used}% used`,
    "aria-valuenow": used,
    "aria-valuemin": 0,
    "aria-valuemax": 100,
  };
}
