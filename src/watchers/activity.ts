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
 *
 * **A key already held keeps its existing object.** Solid's `<For>` keys on object identity,
 * not on the logical key, and every poll builds fresh `freezeRow` results for the whole ring;
 * replacing the held object with an identical snapshot would rebuild and re-animate every row
 * on screen every ten seconds. The one deliberate exception is below.
 */
export function mergeRows(
  existing: readonly ActivityRow[],
  incoming: readonly ActivityRow[]
): ActivityRow[] {
  const byKey = new Map<string, ActivityRow>();
  for (const row of existing) byKey.set(rowKey(row), row);
  for (const row of incoming) {
    const key = rowKey(row);
    const held = byKey.get(key);
    byKey.set(key, held ? keepIdentity(held, row) : row);
  }
  return [...byKey.values()].sort(compareRowsNewestFirst);
}

/**
 * The one metadata update worth losing identity over.
 *
 * A match frozen before its session list had loaded carries no agent and no workgroup, and
 * `sessionName` falls back to the raw id. The same match arriving again once the list is in
 * hand is the one case where replacing the object is an improvement rather than churn.
 * `agentId` is the exact test: every session this window scopes has one, so a null can only
 * mean the session was not found when the row was frozen.
 */
function keepIdentity(held: ActivityRow, incoming: ActivityRow): ActivityRow {
  return held.agentId === null && incoming.agentId !== null ? incoming : held;
}

/**
 * Keep at most `limit` rows per session and drop the rest, preserving the input order.
 *
 * The 500 and 100 limits are not only fetch parameters: without this the frontend grows
 * without a bound the backend has, because the ring caps what a SNAPSHOT returns while the
 * event stream keeps appending for as long as the window stays open. One session can deliver
 * dozens of rows a second, so an afternoon's uptime is hundreds of thousands of objects, a
 * `sort` whose cost climbs with every batch, and a `<For>` rendering all of it. It also makes
 * the truncation the banner announces true of the window and not only of the buffer.
 *
 * **The survivors are chosen by `seq`, never by the order the rows are displayed in.** `at`
 * is the tick's instant and it comes from the machine's clock; let that clock step back --
 * NTP, a manual change, a VM resume -- with the table already full, and every new match
 * arrives sorting BEHIND everything already held. Keeping the first N of the display order
 * would then discard precisely the newest activity, on every event and every snapshot, until
 * the clock caught up to where it had been. `seq` is monotonic per session by contract and
 * has no such failure.
 *
 * The cost is one sort of the seqs of each over-limit session, which is bounded by `limit`
 * plus the batch that pushed it over.
 */
export function capPerSession(
  rows: readonly ActivityRow[],
  limit: number
): ActivityRow[] {
  const seqsBySession = new Map<string, number[]>();
  for (const row of rows) {
    const seqs = seqsBySession.get(row.sessionId);
    if (seqs) seqs.push(row.seq);
    else seqsBySession.set(row.sessionId, [row.seq]);
  }

  // The lowest `seq` that still survives, per session. Absent means that session is under
  // the limit and keeps everything.
  const survivesFrom = new Map<string, number>();
  for (const [sessionId, seqs] of seqsBySession) {
    if (seqs.length <= limit) continue;
    seqs.sort((a, b) => b - a);
    survivesFrom.set(sessionId, seqs[limit - 1]);
  }
  if (survivesFrom.size === 0) return rows.slice();

  // `(sessionId, seq)` is the row key, so no session holds a `seq` twice and this keeps
  // exactly `limit` of them.
  return rows.filter((row) => {
    const threshold = survivesFrom.get(row.sessionId);
    return threshold === undefined || row.seq >= threshold;
  });
}

/**
 * Newest first.
 *
 * Within one session `seq` decides on its own: it is monotonic and exact, while `at` is the
 * TICK's instant, shared by every match of that tick, and its RFC3339 text does not even
 * compare correctly against a fractional-second sibling -- `.` sorts before `Z`, so
 * `...00.100Z` reads as older than `...00Z`. Across sessions there is no shared counter, so
 * time is what is left, with the id as a stable tie-break.
 */
export function compareRowsNewestFirst(a: ActivityRow, b: ActivityRow): number {
  if (a.sessionId === b.sessionId) return b.seq - a.seq;
  if (a.at !== b.at) return a.at < b.at ? 1 : -1;
  return a.sessionId < b.sessionId ? -1 : 1;
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
 *
 * `filtered` is the fifth state and it exists because the other four are statements about the
 * ACTIVATIONS, not about the table. Deciding them from filtered rows means a filter that
 * happens to hide everything produces "Configured and waiting. Nothing has matched yet.",
 * which is false and which the user cannot even attribute to the filter they just set.
 */
export type ActivityView =
  | "warming"
  | "unconfigured"
  | "waiting"
  | "filtered"
  | "rows";

export function resolveView(
  snapshots: readonly WatcherActivitySnapshot[],
  totalRows: number,
  visibleRows: number
): ActivityView {
  if (visibleRows > 0) return "rows";
  // There ARE activations; the filters are what is hiding them. Said before the snapshot is
  // consulted, because no reading of the snapshot can make "nothing has matched" true here.
  if (totalRows > 0) return "filtered";
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

/**
 * The watchers currently hitting a per-tick cap or suspended, across the scope.
 *
 * Surfaced on its own because a degraded watcher has, by definition, already emitted matches:
 * hanging the marker off the "configured and waiting" branch puts it exactly where it can
 * never be reached, since one visible row sends the window to the table instead.
 */
export function degradedWatchers(
  snapshots: readonly WatcherActivitySnapshot[]
): WatcherActivityCounter[] {
  return mergeActiveWatchers(snapshots).filter((counter) => counter.degraded);
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
