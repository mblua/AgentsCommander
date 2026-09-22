import { sessionActivity, type ActivitySession, type SessionActivity } from "../../shared/session-activity";

export type SessionDotClass =
  | "active"
  | "running"
  | "idle"
  | "exited"
  | "comanaged"
  | "pending"
  | "waiting"
  | "offline";

/**
 * #882 Presentation projection: SessionActivity -> CSS dot class. Total and 1:1.
 * Editing a value here changes pixels only; no behavior reads a dot class.
 */
const DOT_CLASS: Record<SessionActivity, SessionDotClass> = {
  offline: "offline",
  exited: "exited",
  comanaged: "comanaged",
  pendingReview: "pending",
  waitingForInput: "waiting",
  active: "active",
  running: "running",
  idle: "idle",
};

export function sessionDotClass(
  session: ActivitySession | null | undefined,
  options: { inactive?: boolean; comanaged?: boolean } = {},
): SessionDotClass {
  return DOT_CLASS[sessionActivity(session, options)];
}
