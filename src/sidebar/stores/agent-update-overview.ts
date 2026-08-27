import { createStore } from "solid-js/store";
import { AgentUpdateAPI } from "../../shared/ipc";
import type { AgentUpdateOverviewRow, InstallState } from "../../shared/types";

/**
 * #1551 - the Settings Auto-update overview (plan 5.11): per-mount freshness (`open` /
 * `close` sessions), a trailing single-flight `refresh`, the sequence-ordered merge of
 * install states from responses and events alike (highest `seq` per command wins, so
 * the result is independent of delivery order), and a bounded re-poll while a response
 * still reports a `checking` row.
 */

/** #1551 - retry/reconciliation interval (PROBE_TIMEOUT 15 s plus a 5 s IPC/UI margin): while the latest
 *  response still reports a `checking` row, the store re-asks the backend once per interval. It is a
 *  re-ask cadence, NOT a completion bound: a re-ask may schedule a replacement probe that itself takes up
 *  to PROBE_TIMEOUT, and while the startup pass is unfinished nothing is scheduled at all. */
export const OVERVIEW_REPOLL_MS = 20_000;

interface OverviewState {
  rows: AgentUpdateOverviewRow[] | null;
  loading: boolean;
  error: string | null;
}

const [state, setState] = createStore<OverviewState>({ rows: null, loading: false, error: null });

// Non-reactive module state.
let session = 0;
/** The highest-`seq` state seen per command in this open, from responses and events alike. */
const latest = new Map<string, InstallState>();
let inFlight: Promise<void> | null = null;
let wanted = false;
let repoll: ReturnType<typeof setTimeout> | null = null;

/** Sequence rule: an incoming state is applied only when its `seq` is higher than the one already seen. */
function noteInstall(command: string, install: InstallState): boolean {
  const cur = latest.get(command);
  if (cur && install.seq <= cur.seq) return false;
  latest.set(command, install);
  return true;
}

function cancelRepoll(): void {
  if (repoll !== null) {
    clearTimeout(repoll);
    repoll = null;
  }
}

/** Armed only by responses; cancelled by `open()` / `close()` / the next response. */
function armRepoll(needed: boolean): void {
  cancelRepoll();
  if (needed) {
    repoll = setTimeout(() => {
      repoll = null;
      void refresh();
    }, OVERVIEW_REPOLL_MS);
  }
}

function applyResponse(rows: AgentUpdateOverviewRow[]): void {
  for (const row of rows) noteInstall(row.command, row.install);
  setState({
    rows: rows.map((row) => ({ ...row, install: latest.get(row.command) ?? row.install })),
    error: null,
  });
  // The test is on the RESPONSE rows: a `checking` there means a probe is pending or
  // deferred, even when the displayed cell keeps an older committed state.
  armRepoll(rows.some((row) => row.install.status === "checking"));
}

/**
 * Trailing single-flight: every call is followed by an invoke that STARTS after the call;
 * concurrent callers never produce more than one in-flight invoke; rows are kept on failure.
 */
function refresh(): Promise<void> {
  wanted = true;
  if (inFlight) return inFlight;
  inFlight = (async () => {
    try {
      while (wanted) {
        wanted = false;
        const mySession = session;
        setState("loading", true);
        try {
          const rows = await AgentUpdateAPI.getOverview();
          if (mySession === session) applyResponse(rows);
        } catch (err) {
          if (mySession === session) setState("error", err instanceof Error ? err.message : String(err));
        } finally {
          if (mySession === session) setState("loading", false);
        }
      }
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

export const agentUpdateOverviewStore = {
  /** Read-only accessor for components. */
  state,

  /** One Coding Agents screen mount: a fresh session, an empty `latest` map, no rows yet. */
  open(): void {
    session += 1;
    latest.clear();
    cancelRepoll();
    setState({ rows: null, loading: false, error: null });
  },

  /** The mount is gone: a response still in flight is discarded on arrival. */
  close(): void {
    session += 1;
    cancelRepoll();
  },

  refresh,

  /** Event path: applied to every row with that command (duplicate-command rows move together). */
  applyInstallState(command: string, install: InstallState): void {
    if (!noteInstall(command, install)) return;
    if (state.rows) {
      // Row-level object write on purpose: the path form `("rows", filter, "install", install)`
      // would MERGE the new state into the old one and keep keys the new state omits
      // (`version`/`path` of a stale `installed`); `{ install }` replaces the entry.
      setState("rows", (row) => row.command === command, { install });
    }
  },

  /** Only between tests. */
  resetForTests(): void {
    this.close();
    this.open();
    inFlight = null;
    wanted = false;
  },
};
