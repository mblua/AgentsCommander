import type { Session, SessionStatus } from "../../shared/types";

/**
 * True when `status` is one of the runtime string states ("active" | "running"
 * | "idle"). False when it is an Exited({ exited: N }) object.
 *
 * Used by sessionsStore.setActiveId to skip Exited sessions when promoting
 * the selected session to "active": a dormant root must keep its
 * { exited: N } status so RootAgentBanner's wake path
 * (typeof status !== "string") still fires and chooses
 * restart(..., { skipAutoResume: false }) - i.e. wake-with-provider-resume.
 */
export function isRuntimeStringStatus(status: SessionStatus): boolean {
  return typeof status === "string";
}

/**
 * Upsert `incoming` into `prev` by id. If the id is already present, merge
 * `incoming`'s fields onto the existing entry (incoming wins). Otherwise
 * append.
 *
 * Used by sessionsStore.addSession so the RootAgentBanner can hydrate the
 * store with the Session returned from createRootAgent/restart even when the
 * backend reuses an existing live root and therefore does NOT emit a fresh
 * session_created event (see src-tauri/src/commands/session.rs ReuseLive).
 */
export function upsertSessionList(prev: Session[], incoming: Session): Session[] {
  const idx = prev.findIndex((s) => s.id === incoming.id);
  if (idx === -1) return [...prev, incoming];
  const next = prev.slice();
  next[idx] = { ...prev[idx], ...incoming };
  return next;
}
