import type { Session, SessionStatus } from "./types";

export type SessionActivity =
  | "offline"
  | "exited"
  | "comanaged"
  | "pendingReview"
  | "waitingForInput"
  | "active"
  | "running"
  | "idle";

export type ActivitySession = Pick<Session, "status" | "pendingReview" | "waitingForInput">;

export function sessionRuntimeState(
  status: SessionStatus
): "active" | "running" | "idle" | "exited" {
  if (typeof status === "string") return status;
  return "exited";
}

export function sessionActivity(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean; comanaged?: boolean } = {}
): SessionActivity {
  if (!session || options.inactive) return "offline";
  const runtime = sessionRuntimeState(session.status);
  if (runtime === "exited") return "exited";
  // #2271 - comanaged beats pendingReview/waitingForInput on purpose: the idle
  // edge is exactly where those would otherwise light up, and a Co-managed
  // session must never paint idle. `exited` still wins above.
  if (options.comanaged) return "comanaged";
  if (session.pendingReview) return "pendingReview";
  if (session.waitingForInput) return "waitingForInput";
  return runtime;
}

export function isWorkingActivity(activity: SessionActivity): boolean {
  return activity === "active" || activity === "running";
}

export function isSessionWorking(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean; comanaged?: boolean } = {}
): boolean {
  return isWorkingActivity(sessionActivity(session, options));
}
