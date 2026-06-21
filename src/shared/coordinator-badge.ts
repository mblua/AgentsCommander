import type { AppSettings } from "./types";

export type CoordinatorBadgeColor = "green" | "yellow" | "red";

export interface CoordinatorBadge {
  /** Minutes-only label, e.g. "0m", "45m", "4320m". */
  label: string;
  /** Threshold color class applied to the badge element. */
  colorClass: CoordinatorBadgeColor;
}

/**
 * #552/#580 Pure, timer-free badge derivation so it unit-tests cleanly. The
 * input timestamp is the unified team-idle anchor (#580): minutes since the team
 * was last truly active, not merely the user's last message (the param name is
 * retained — rename deferred). Minutes-only, raw integer, NO h/d rollover (spec):
 * 45m, 135m, 4320m. Color comes from the global thresholds: `minutes >= red ->
 * red`, else `>= yellow -> yellow`, else green (red is tested first, so an
 * inverted yellow>=red pair just collapses the yellow band rather than
 * misbehaving). The `Math.max(0, ...)` floor means clock skew / a future-dated
 * timestamp shows `0m`, never a negative value.
 *
 * Returns `null` when there is no usable timestamp (absent or unparseable), so
 * callers render nothing for a coordinator with no idle anchor yet.
 */
export function coordinatorIdleBadge(
  lastUserMessageAtIso: string | undefined,
  nowMs: number,
  settings: Pick<
    AppSettings,
    "coordinatorIdleBadgeYellowMinutes" | "coordinatorIdleBadgeRedMinutes"
  > | null
): CoordinatorBadge | null {
  if (!lastUserMessageAtIso) return null;
  const then = Date.parse(lastUserMessageAtIso);
  if (Number.isNaN(then)) return null;
  const minutes = Math.max(0, Math.floor((nowMs - then) / 60_000));
  const yellow = settings?.coordinatorIdleBadgeYellowMinutes ?? 30;
  const red = settings?.coordinatorIdleBadgeRedMinutes ?? 60;
  const colorClass: CoordinatorBadgeColor =
    minutes >= red ? "red" : minutes >= yellow ? "yellow" : "green";
  return { label: `${minutes}m`, colorClass };
}
