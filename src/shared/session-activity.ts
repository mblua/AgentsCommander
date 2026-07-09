import type { Session, SessionStatus } from "./types";

/**
 * #882 Domain classification of what a session is doing. This is NOT a CSS class:
 * sessionDotClass() is a 1:1 projection of this enum onto dot class names, and
 * it is the only place a presentation value is produced. Behavior reads this
 * enum, never the dot class.
 *
 * The precedence chain is a domain statement, not a visual one:
 *   offline > exited > pendingReview > waitingForInput > status(active|running|idle)
 *
 * DO NOT REORDER pendingReview AND waitingForInput. sessions.ts lowers the two
 * flags in separate unbatched setState calls (#886), so the store is briefly
 * observable as waitingForInput=false, pendingReview=true. Reading pendingReview
 * first means a pending session's consumers short-circuit before ever reading
 * waitingForInput, never subscribe to it, and are never woken by that first
 * write. The order is load-bearing; session-activity.test.ts pins it.
 */
export type SessionActivity =
  | "offline"
  | "exited"
  | "pendingReview"
  | "waitingForInput"
  | "active"
  | "running"
  | "idle";

/** The subset of Session the classifier reads. */
export type ActivitySession = Pick<Session, "status" | "pendingReview" | "waitingForInput">;

/** Collapse the tagged Exited(code) variant onto a flat runtime state. */
export function sessionRuntimeState(
  status: SessionStatus
): "active" | "running" | "idle" | "exited" {
  if (typeof status === "string") return status;
  return "exited";
}

export function sessionActivity(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean } = {}
): SessionActivity {
  if (!session || options.inactive) return "offline";
  const runtime = sessionRuntimeState(session.status);
  if (runtime === "exited") return "exited";
  if (session.pendingReview) return "pendingReview";
  if (session.waitingForInput) return "waitingForInput";
  return runtime;
}

/**
 * #882 The load-bearing predicate. Working means a live session whose agent is
 * currently progressing on its own. Neither waitingForInput nor pendingReview
 * counts as working.
 *
 * pendingReview is not redundant with waitingForInput here. At rest it implies
 * it, but not mid-write; dropping it breaks the existing rail pending test.
 */
export function isWorkingActivity(activity: SessionActivity): boolean {
  return activity === "active" || activity === "running";
}

export function isSessionWorking(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean } = {}
): boolean {
  return isWorkingActivity(sessionActivity(session, options));
}
