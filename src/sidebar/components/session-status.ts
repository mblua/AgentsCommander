import type { Session, SessionStatus } from "../../shared/types";

export type SessionDotClass =
  | "active"
  | "running"
  | "idle"
  | "exited"
  | "pending"
  | "waiting"
  | "offline";

type DotSession = Pick<Session, "status" | "pendingReview" | "waitingForInput">;

export function sessionStatusClass(status: SessionStatus): "active" | "running" | "idle" | "exited" {
  if (typeof status === "string") return status;
  return "exited";
}

export function sessionDotClass(
  session: DotSession | null | undefined,
  options: { inactive?: boolean } = {},
): SessionDotClass {
  if (!session || options.inactive) return "offline";

  const status = sessionStatusClass(session.status);
  if (status === "exited") return "exited";
  if (session.pendingReview) return "pending";
  if (session.waitingForInput) return "waiting";
  return status;
}
