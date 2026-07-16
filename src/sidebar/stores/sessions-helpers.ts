import type { Session, SessionSelection, SessionStatus } from "../../shared/types";

/**
 * True when `status` is one of the runtime string states ("active" | "running"
 * | "idle"). False when it is an Exited({ exited: N }) object.
 *
 * Used by authoritative selection projection to skip Exited sessions when
 * applying a stored live payload: a dormant root must keep its
 * { exited: N } status so RootAgentBanner's wake path
 * (typeof status !== "string") still fires and chooses
 * restart(..., { skipAutoResume: false }) — i.e. wake-with-provider-resume.
 */
export function isRuntimeStringStatus(status: SessionStatus): boolean {
  return typeof status === "string";
}

export interface AppliedSessionSelection {
  sessions: Session[];
  activeId: string | null;
}

/** Pure sidebar projection of one already-decoded authoritative selection. */
export function applySelectionToSessionList(
  sessions: Session[],
  selection: SessionSelection,
): AppliedSessionSelection {
  const target = selection.id
    ? sessions.find((session) => session.id === selection.id)
    : undefined;
  const targetCanBeLive =
    selection.mode === "live" &&
    !!target &&
    typeof target.status === "string";
  const activeId = selection.mode === "dormant"
    ? target?.id ?? null
    : targetCanBeLive
      ? selection.id
      : null;

  return {
    activeId,
    sessions: sessions.map((session) => {
      let status = session.status;
      if (session.id === selection.id) {
        if (selection.mode === "dormant") status = { ...selection.status };
        else if (selection.mode === "live" && typeof session.status === "string") status = "active";
      } else if (status === "active") {
        status = "running";
      }
      if (status === session.status && (session.id !== selection.id || !session.pendingReview)) {
        return session;
      }
      return {
        ...session,
        status,
        pendingReview: session.id === selection.id ? false : session.pendingReview,
      };
    }),
  };
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

/**
 * Keep the previous visible order while allowing removals and appending new
 * items. Used to prevent activity-driven coordinator sorting from moving rows
 * under the pointer during Sidebar interaction.
 */
export function preserveVisibleOrder<T>(
  next: T[],
  previous: T[] | undefined,
  keyFor: (item: T) => string,
): T[] {
  if (!previous || previous.length === 0) return next;

  const nextByKey = new Map(next.map((item) => [keyFor(item), item]));
  const used = new Set<string>();
  const ordered: T[] = [];

  for (const previousItem of previous) {
    const key = keyFor(previousItem);
    const nextItem = nextByKey.get(key);
    if (!nextItem) continue;
    ordered.push(nextItem);
    used.add(key);
  }

  for (const nextItem of next) {
    const key = keyFor(nextItem);
    if (used.has(key)) continue;
    ordered.push(nextItem);
  }

  return ordered;
}

/**
 * Reconcile a frozen visible key order with a freshly recomputed key set:
 * existing keys keep their frozen positions, disappeared keys are removed, and
 * newly visible keys append in the recomputed order.
 */
export function reconcileVisibleOrderKeys(nextKeys: string[], frozenKeys: string[] | undefined): string[] {
  if (!frozenKeys || frozenKeys.length === 0) return nextKeys;

  const nextKeySet = new Set(nextKeys);
  const used = new Set<string>();
  const ordered: string[] = [];

  for (const key of frozenKeys) {
    if (!nextKeySet.has(key)) continue;
    ordered.push(key);
    used.add(key);
  }

  for (const key of nextKeys) {
    if (used.has(key)) continue;
    ordered.push(key);
  }

  return ordered;
}
