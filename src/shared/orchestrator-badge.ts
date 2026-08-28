import type { AppSettings } from "./types";

export type CoordinatorIdleLevel = "ok" | "warn" | "stale";

export interface CoordinatorBadge {
  label: string;
  level: CoordinatorIdleLevel;
}

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
  const level: CoordinatorIdleLevel =
    minutes >= red ? "stale" : minutes >= yellow ? "warn" : "ok";
  return { label: `${minutes}m`, level };
}
