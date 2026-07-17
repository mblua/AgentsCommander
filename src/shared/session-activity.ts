import type { Session, SessionStatus } from "./types";

export type SessionActivity =
  | "offline"
  | "exited"
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
  options: { inactive?: boolean } = {}
): SessionActivity {
  if (!session || options.inactive) return "offline";
  const runtime = sessionRuntimeState(session.status);
  if (runtime === "exited") return "exited";
  if (session.pendingReview) return "pendingReview";
  if (session.waitingForInput) return "waitingForInput";
  return runtime;
}

export function isWorkingActivity(activity: SessionActivity): boolean {
  return activity === "active" || activity === "running";
}

export function isSessionWorking(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean } = {}
): boolean {
  return isWorkingActivity(sessionActivity(session, options));
}
