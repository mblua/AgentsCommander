import { extractWorkgroupName } from "../shared/path-extractors";
import type {
  Session,
  WatcherActivityCounter,
  WatcherActivitySnapshot,
  WatcherMatchPayload,
  WatcherMode,
} from "../shared/types";

/**
 * #1171 - the pure half of the activity window: merging, freezing and filtering.
 *
 * Kept out of the TSX so the merge in particular can be tested directly. It is the piece
 * that decides whether a match arriving while the snapshot is in flight is shown once,
 * twice, or not at all.
 */

/** How many matches to ask for per session, per scope. */
export const SINGLE_SESSION_LIMIT = 500;
export const ALL_SESSIONS_LIMIT = 100;

/**
 * One table row: a match plus the labels resolved when it was inserted.
 *
 * The labels are frozen because a row must survive its session's death still saying which
 * session, agent and workgroup it came from. The agent label is re-resolved live for
 * display, with the frozen value as the fallback, so renaming an entry does not leave rows
 * lying about it.
 */
export interface ActivityRow {
  sessionId: string;
  seq: number;
  watcherId: string;
  mode: WatcherMode;
  at: string;
  captures: (string | null)[];
  row: string;
  rowTruncated: boolean;
  sessionName: string;
  agentId: string | null;
  frozenAgentLabel: string | null;
  workgroup: string | null;
}

/** The identity of a row. `at` cannot carry it: it is the tick's instant, so two matches
 *  from one tick share it, and with `dedupe: "none"` they can be identical in every other
 *  field too. `seq` is the only thing that separates them. */
export function rowKey(row: Pick<ActivityRow, "sessionId" | "seq">): string {
  return `${row.sessionId}:${row.seq}`;
}

/** Resolve and freeze the labels of one match. */
export function freezeRow(
  payload: WatcherMatchPayload,
  session: Session | undefined
): ActivityRow {
  return {
    sessionId: payload.sessionId,
    seq: payload.seq,
    watcherId: payload.watcherId,
    mode: payload.mode,
    at: payload.at,
    captures: payload.captures,
    row: payload.row,
    rowTruncated: payload.rowTruncated,
    sessionName: session?.name ?? payload.sessionId,
    agentId: session?.agentId ?? null,
    frozenAgentLabel: session?.agentLabel ?? null,
    // The backend derives the pair from the spawn cwd and says in writing that it is "not
    // the user-renamable session.name", so this follows the same source.
    workgroup: session ? extractWorkgroupName(session.workingDirectory) : null,
  };
}

/**
 * Merge new rows into the ones already held, newest first.
 *
 * The window subscribes before it fetches, so the snapshot and the stream overlap on
 * purpose: losing that overlap would lose matches, and not deduplicating it would show them
 * twice. Keying on `(sessionId, seq)` makes the overlap exact rather than heuristic.
 */
export function mergeRows(
  existing: readonly ActivityRow[],
  incoming: readonly ActivityRow[]
): ActivityRow[] {
  const byKey = new Map<string, ActivityRow>();
  for (const row of existing) byKey.set(rowKey(row), row);
  for (const row of incoming) byKey.set(rowKey(row), row);
  return [...byKey.values()].sort(compareRowsNewestFirst);
}

/** Newest first; `seq` breaks the tie between two matches of the same tick. */
export function compareRowsNewestFirst(a: ActivityRow, b: ActivityRow): number {
  if (a.at !== b.at) return a.at < b.at ? 1 : -1;
  if (a.sessionId !== b.sessionId) return a.sessionId < b.sessionId ? -1 : 1;
  return b.seq - a.seq;
}

/** Drop rows whose session left the scope, so switching scope does not keep stale rows. */
export function keepSessions(
  rows: readonly ActivityRow[],
  sessionIds: readonly string[]
): ActivityRow[] {
  const kept = new Set(sessionIds);
  return rows.filter((row) => kept.has(row.sessionId));
}

export interface ActivityFilters {
  watchers: ReadonlySet<string>;
  agents: ReadonlySet<string>;
  workgroups: ReadonlySet<string>;
  text: string;
}

/** AND between dimensions, OR within one, matching the Resource Monitor's filter shape. */
export function filterRows(
  rows: readonly ActivityRow[],
  filters: ActivityFilters
): ActivityRow[] {
  const needle = filters.text.trim().toLowerCase();
  return rows.filter((row) => {
    if (filters.watchers.size > 0 && !filters.watchers.has(row.watcherId)) return false;
    if (filters.agents.size > 0 && !(row.agentId && filters.agents.has(row.agentId))) {
      return false;
    }
    if (
      filters.workgroups.size > 0 &&
      !(row.workgroup && filters.workgroups.has(row.workgroup))
    ) {
      return false;
    }
    if (needle) {
      const haystack = row.captures
        .filter((capture): capture is string => capture !== null)
        .join(" ")
        .toLowerCase();
      if (!haystack.includes(needle)) return false;
    }
    return true;
  });
}

/** Distinct values in first-seen order, the Resource Monitor's `distinct`. */
export function distinct<T>(values: readonly T[]): T[] {
  return [...new Set(values)];
}

/**
 * What the window is showing right now, decided from snapshot VALUES with no nullability.
 *
 * `warming` is its own state and not empty state 1: until the engine has ticked a session
 * once, an empty `activeWatchers` means "not known yet", and showing "no watcher reaches
 * this agent" for the first 200 ms of every session would be a lie that flickers.
 */
export type ActivityView = "warming" | "unconfigured" | "waiting" | "rows";

export function resolveView(
  snapshots: readonly WatcherActivitySnapshot[],
  visibleRows: number
): ActivityView {
  if (visibleRows > 0) return "rows";
  if (snapshots.length === 0 || !snapshots.some((snapshot) => snapshot.warmedUp)) {
    return "warming";
  }
  const active = snapshots.flatMap((snapshot) => snapshot.activeWatchers);
  return active.length === 0 ? "unconfigured" : "waiting";
}

/** The reaching watchers across every session in scope, one entry per id. */
export function mergeActiveWatchers(
  snapshots: readonly WatcherActivitySnapshot[]
): WatcherActivityCounter[] {
  const byId = new Map<string, WatcherActivityCounter>();
  for (const snapshot of snapshots) {
    for (const counter of snapshot.activeWatchers) {
      const seen = byId.get(counter.watcherId);
      byId.set(
        counter.watcherId,
        seen
          ? {
              ...seen,
              count: seen.count + counter.count,
              degraded: seen.degraded || counter.degraded,
            }
          : { ...counter }
      );
    }
  }
  return [...byId.values()].sort((a, b) => a.watcherId.localeCompare(b.watcherId));
}

/** True when any session in scope dropped entries, i.e. the table is missing older rows. */
export function anyTruncated(snapshots: readonly WatcherActivitySnapshot[]): boolean {
  return snapshots.some((snapshot) => snapshot.truncated);
}

/** Frames the sampler could not align, summed across the scope. Not a count of lost
 *  matches: above zero it means "something may have been missed", never how much. */
export function totalPossiblyMissedFrames(
  snapshots: readonly WatcherActivitySnapshot[]
): number {
  return snapshots.reduce((total, snapshot) => total + snapshot.possiblyMissedFrames, 0);
}
