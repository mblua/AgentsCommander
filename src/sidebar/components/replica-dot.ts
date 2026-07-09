import type { AcAgentReplica, AcWorkgroup } from "../../shared/types";
import { sessionDotClass, type SessionDotClass } from "./session-status";
import { findReplicaSession } from "./workgroup-session";

/**
 * #882 Render-boundary helper. Lives outside workgroup-session.ts so that the
 * watchdog's import graph never reaches SessionDotClass. Consumers must be render
 * code only; anything asking "is this replica working?" calls isReplicaWorking.
 */
export function replicaDotClass(wg: AcWorkgroup, replica: AcAgentReplica): SessionDotClass {
  return sessionDotClass(findReplicaSession(wg, replica));
}
