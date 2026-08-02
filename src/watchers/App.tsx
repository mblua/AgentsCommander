import {
  Component,
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
} from "solid-js";
import {
  PtyAPI,
  SessionAPI,
  WindowAPI,
  emitOpenSettings,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
  onWatcherMatches,
  onWatchersScopeRequest,
} from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { settingsStore } from "../shared/stores/settings";
import { formatClockTime } from "../shared/time-format";
import type { UnlistenFn } from "../shared/transport";
import type { Session, WatcherActivitySnapshot, WindowGeometry } from "../shared/types";
import {
  ALL_SESSIONS_LIMIT,
  SINGLE_SESSION_LIMIT,
  type ActivityRow,
  anyTruncated,
  capPerSession,
  degradedWatchers,
  distinct,
  filterRows,
  freezeRow,
  keepSessions,
  mergeActiveWatchers,
  mergeRows,
  resolveView,
  rowKey,
  totalPossiblyMissedFrames,
} from "./activity";
import WatchersTitlebar from "./components/WatchersTitlebar";
import "./styles/watchers.css";

/** Snapshot poll cadence. The snapshot is the ONLY carrier of `truncated`,
 *  `possiblyMissedFrames`, `warmedUp`, `activeWatchers` and `degraded`, and there is no
 *  cross-window settings event to hang a refresh on, so without a poll those five freeze at
 *  their mount values. Cadence copies the Resource Monitor's written precedent
 *  (`ActionBar.tsx:81-87`). */
export const POLL_FOCUSED_MS = 10_000;
export const POLL_UNFOCUSED_MS = 15_000;

/** How long one activity round may take before it is abandoned.
 *
 *  #1188: a `finally` on the round is not enough on its own, because a promise that never
 *  settles never reaches `finally` either. This deadline is what turns "never settles" into
 *  "settles as a failure", and every re-arm below depends on it.
 *
 *  8s against a command measured at 13ms in the worst stressed case, so no plausible fan-out
 *  makes it fire on a merely slow round. It sits under POLL_FOCUSED_MS purely as a service
 *  level: a hung round is reported within one period instead of after several. It is NOT
 *  what stops rounds overlapping. Nothing can overlap here at any value, because the chain
 *  arms the next round only once this one has ended. Exported for the invariant test. */
export const POLL_TIMEOUT_MS = 8_000;

/** What the window says when a round is abandoned. Exported so the regression test asserts
 *  the exact string the window paints, rather than a substring that could drift. */
export const POLL_TIMEOUT_MESSAGE =
  "Activity refresh timed out. The list may be out of date; retrying.";

/** How long a move or resize settles before the rect is persisted. */
const GEOMETRY_SAVE_DEBOUNCE_MS = 500;

/** How close to the top still counts as pinned, in pixels. */
const PIN_TO_TOP_SLACK_PX = 8;

/**
 * Physical device pixels to the logical ones the window builder takes.
 *
 * `outerPosition()` and `innerSize()` answer in `Physical*`, while `WebviewWindowBuilder`'s
 * `position` and `inner_size` are documented as logical, so persisting what was read would
 * multiply the rect by the scale factor on every save-and-reopen cycle: at 150% DPI a
 * 1470-wide window reopens 2205 wide, then 3307, and walks off the screen. Rounded because
 * `WindowGeometry` is integral on both sides of the IPC and a half pixel means nothing here.
 *
 * Exported for the conversion test: it is pure arithmetic and the defect it fixes is
 * invisible at a scale factor of 1, which is the only one a headless run ever has.
 */
export function logicalGeometry(
  physical: WindowGeometry,
  scaleFactor: number
): WindowGeometry {
  // A zero or absent factor would turn every coordinate into Infinity or NaN. Treating an
  // impossible factor as 1 keeps the previous rect rather than persisting nonsense.
  const factor = scaleFactor > 0 ? scaleFactor : 1;
  return {
    x: Math.round(physical.x / factor),
    y: Math.round(physical.y / factor),
    width: Math.round(physical.width / factor),
    height: Math.round(physical.height / factor),
  };
}

/**
 * Register a set of listeners as one unit, and hand back a single release for all of them.
 *
 * Registering them one at a time with each unlisten in its own local is what leaks: a later
 * registration that REJECTS leaves the earlier one alive and unreachable, attached to a
 * window that is on its way out, because nothing ever puts it where the cleanup can see it.
 * Here everything that succeeded is held in one place and the `finally` releases it on every
 * path that does not hand it over -- a rejection, or the window closing on any await.
 *
 * `null` means the set was released rather than handed over, so the caller owns nothing.
 * Exported for its own test: the failure it exists for needs a registration that rejects,
 * which no real Tauri window produces on demand.
 */
export async function registerAll(
  registrations: readonly (() => Promise<UnlistenFn>)[],
  isDisposed: () => boolean
): Promise<(() => void) | null> {
  const registered: UnlistenFn[] = [];
  const release = () => {
    for (const unlisten of registered.splice(0)) unlisten();
  };
  let handedOver = false;
  try {
    for (const registration of registrations) {
      if (isDisposed()) return null;
      registered.push(await registration());
    }
    // Checked again after the last await, which the loop's own guard never reaches.
    if (isDisposed()) return null;
    handedOver = true;
    return release;
  } finally {
    if (!handedOver) release();
  }
}

/**
 * Resolve with `work`, or reject with `message` once `ms` have elapsed, whichever is first.
 *
 * The rejection is the whole point. `work` is a Tauri `invoke`, nothing can cancel it, and a
 * lost reply leaves it pending forever; #1188 is that promise taking the only re-arm of the
 * poll down with it. Racing does not stop the call, it stops the WAIT, which is what every
 * `finally` downstream needs in order to run at all.
 *
 * `Promise.race` keeps a handler attached to `work`, so a late rejection of an abandoned call
 * is absorbed here instead of escaping as an unhandled one. The timer is cleared on both
 * paths, so a healthy round leaves nothing armed.
 *
 * Exported for its own test: the failure it exists for needs a promise that never settles,
 * which no real IPC call produces on demand.
 */
export function withDeadline<T>(
  work: Promise<T>,
  ms: number,
  message: string
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([work, deadline]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

const WatchersApp: Component<{ initialSessionId?: string }> = (props) => {
  // ── Lifecycle ──────────────────────────────────────────────────────────────
  // `onMount` crosses several awaits and the window can be closed inside any of them, so
  // every registration goes through `register`, which unlistens immediately when its
  // continuation lands after teardown, and every await is followed by a `disposed` check.
  // The shape is `terminal/App.tsx`'s. Without it a fast close leaves listeners, a fetch and
  // a poll running against a component that is already gone.
  const listeners: UnlistenFn[] = [];
  let disposed = false;
  let cleanupGeometry: (() => void) | null = null;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  const mountDisposed = Symbol("watchersMountDisposed");

  const register = async (registration: Promise<UnlistenFn>): Promise<void> => {
    const unlisten = await registration;
    if (disposed) {
      unlisten();
      throw mountDisposed;
    }
    listeners.push(unlisten);
  };

  // `null` is the "All sessions" scope. The query parameter is the INITIAL value only, so the
  // first paint has a scope without waiting for a round trip; the pull below is authoritative.
  const [scopeSessionId, setScopeSessionId] = createSignal<string | null>(
    props.initialSessionId ?? null
  );
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [rows, setRows] = createSignal<ActivityRow[]>([]);
  const [snapshots, setSnapshots] = createSignal<WatcherActivitySnapshot[]>([]);
  const [loadError, setLoadError] = createSignal("");
  /** False until the mount has settled the scope. Nothing may be fetched before it. */
  const [scopeSettled, setScopeSettled] = createSignal(false);

  const [watcherFilter, setWatcherFilter] = createSignal<Set<string>>(new Set());
  const [agentFilter, setAgentFilter] = createSignal<Set<string>>(new Set());
  const [workgroupFilter, setWorkgroupFilter] = createSignal<Set<string>>(new Set());
  const [textFilter, setTextFilter] = createSignal("");
  const [expandedKey, setExpandedKey] = createSignal<string | null>(null);

  let scrollEl: HTMLDivElement | undefined;
  let scopeEl: HTMLSelectElement | undefined;
  const [pinnedTop, setPinnedTop] = createSignal(true);

  // Only agent sessions are ever registered with the engine, exactly as with the context
  // scraper. A plain shell has a screen but never a watcher.
  const agentSessions = createMemo(() => sessions().filter((s) => !!s.agentId));
  const sessionById = (id: string) => sessions().find((s) => s.id === id);

  const scopeIds = createMemo(() => {
    const single = scopeSessionId();
    if (single) return [single];
    return agentSessions().map((s) => s.id);
  });
  const isAllSessions = () => scopeSessionId() === null;
  const scopeLimit = () => (isAllSessions() ? ALL_SESSIONS_LIMIT : SINGLE_SESSION_LIMIT);

  /** Live agent label by id, falling back to what was frozen into the row, so renaming an
   *  entry updates old rows instead of leaving them lying. Precedent: `liveAgentLabel`. */
  const agentLabel = (row: ActivityRow): string => {
    const live = row.agentId
      ? settingsStore.current?.agents?.find((a) => a.id === row.agentId)?.label
      : null;
    return live || row.frozenAgentLabel || row.agentId || "";
  };

  // The window loads its own settings and never applied the theme, so a user on light got a
  // dark window. Hung off the store rather than a mount-time read because the poll already
  // refreshes it, which makes a theme change land within one poll instead of at the next open.
  createEffect(() => {
    document.documentElement.classList.toggle(
      "light-theme",
      !!settingsStore.current?.themeLight
    );
  });

  // The scope select is driven by an effect rather than by `value=`, because the window
  // opens scoped to a session whose `<option>` does not exist yet: the list arrives with
  // `list_sessions`, and a `<select>` silently ignores a value it has no option for. Without
  // re-applying it once the options land, the control would read "All sessions" while the
  // window was in fact scoped to one.
  createEffect(() => {
    const desired = scopeSessionId() ?? "all";
    const options = agentSessions();
    if (!scopeEl) return;
    if (desired === "all" || options.some((s) => s.id === desired)) {
      scopeEl.value = desired;
    }
  });

  const visibleRows = createMemo(() =>
    filterRows(rows(), {
      watchers: watcherFilter(),
      agents: agentFilter(),
      workgroups: workgroupFilter(),
      text: textFilter(),
    })
  );

  // Resolved from the UNFILTERED count as well as the visible one: the empty states are
  // statements about the activations, so a filter that hides everything must not be able to
  // say "nothing has matched yet".
  const view = createMemo(() => resolveView(snapshots(), rows().length, visibleRows().length));
  const activeWatchers = createMemo(() => mergeActiveWatchers(snapshots()));
  const degraded = createMemo(() => degradedWatchers(snapshots()));
  const truncated = createMemo(() => anyTruncated(snapshots()));
  const missedFrames = createMemo(() => totalPossiblyMissedFrames(snapshots()));

  const watcherOptions = createMemo(() => distinct(rows().map((r) => r.watcherId)).sort());
  const agentOptions = createMemo(() => {
    const seen = new Map<string, string>();
    for (const row of rows()) {
      if (row.agentId && !seen.has(row.agentId)) seen.set(row.agentId, agentLabel(row));
    }
    return [...seen.entries()].map(([id, label]) => ({ id, label }));
  });
  const workgroupOptions = createMemo(() =>
    distinct(
      rows()
        .map((r) => r.workgroup)
        .filter((wg): wg is string => !!wg)
    ).sort()
  );
  // A label the user wrote can repeat while an id cannot, so a duplicate gets a short id
  // suffix rather than two chips that read the same.
  const agentChipText = (option: { id: string; label: string }) => {
    const duplicate = agentOptions().filter((o) => o.label === option.label).length > 1;
    return duplicate ? `${option.label} (${option.id.slice(0, 6)})` : option.label;
  };

  const filtersActive = () =>
    watcherFilter().size > 0 ||
    agentFilter().size > 0 ||
    workgroupFilter().size > 0 ||
    textFilter().trim() !== "";

  const clearFilters = () => {
    setWatcherFilter(new Set<string>());
    setAgentFilter(new Set<string>());
    setWorkgroupFilter(new Set<string>());
    setTextFilter("");
  };

  const toggleFilter = (
    get: () => Set<string>,
    set: (next: Set<string>) => void,
    value: string
  ) => {
    const next = new Set(get());
    if (next.has(value)) next.delete(value);
    else next.add(value);
    set(next);
  };

  /**
   * One monotonic counter over every fetch, and the only thing that decides whether a
   * response may paint.
   *
   * It covers the scope selector, the re-scope event, the adoption of the pull, the mount's
   * first fetch and the poll, and it also covers two fetches of the SAME scope racing each
   * other, which a scope-keyed generation lets through: the older poll response would
   * otherwise commit an older `lastSeq`, `warmedUp`, `degraded` and counters over the newer
   * ones, leaving the table and the counters describing two different instants.
   */
  let requestCounter = 0;

  const refresh = async () => {
    const request = (requestCounter += 1);
    const ids = scopeIds();
    const limit = scopeLimit();
    try {
      // #1188: the deadline goes INSIDE the map, not around the `Promise.all`. The two read
      // the same and differ only in what a lost reply keeps alive: an aggregate with one
      // element still pending holds its values list, and every already-resolved sibling
      // snapshot in it, reachable for the life of the window. Deadlining each element makes
      // every element settle, so the aggregate settles and releases them.
      const fetched = await Promise.all(
        ids.map((id) =>
          withDeadline(
            PtyAPI.getWatcherActivity(id, limit),
            POLL_TIMEOUT_MS,
            POLL_TIMEOUT_MESSAGE
          )
        )
      );
      if (disposed || request !== requestCounter) return;
      setSnapshots(fetched);
      setLoadError("");
      const incoming = fetched.flatMap((snapshot, index) =>
        snapshot.matches.map((match) => freezeRow(match, sessionById(ids[index])))
      );
      setRows((prev) => capPerSession(mergeRows(keepSessions(prev, ids), incoming), limit));
    } catch (err) {
      if (disposed || request !== requestCounter) return;
      setLoadError(err instanceof Error ? err.message : String(err));
    }
  };

  /**
   * The same monotonic rule as the activity fetch, and for a worse reason.
   *
   * The three session listeners each fire their own `list_sessions`, unserialized, and
   * nothing polls the list afterwards. So an older answer that resolves last does not cause a
   * flicker: it resurrects a session that was destroyed, or removes one that was just
   * created, and the window stays wrong until it is reopened. In "All sessions" that list
   * also IS the scope, so a wrong list is a wrong set of fetches.
   */
  let sessionsRequestCounter = 0;

  const reloadSessions = async () => {
    const request = (sessionsRequestCounter += 1);
    try {
      const listed = await SessionAPI.list();
      if (disposed || request !== sessionsRequestCounter) return;
      setSessions(listed);
    } catch (err) {
      if (disposed || request !== sessionsRequestCounter) return;
      console.error("[watchers] failed to list sessions:", err);
    }
  };

  /**
   * The scope-adoption guard, which is NOT the request counter.
   *
   * This one decides which scope is adopted; that one decides which fetch may paint. They
   * protect different things and each leaves the other's failure open, so they must not be
   * merged.
   */
  let scopeEventGeneration = 0;

  /**
   * What a fetch is actually parameterised by: the ids AND the per-session limit.
   *
   * Stated over `scopeIds()` rather than over a list of callers, because every enumeration of
   * "the callers that must refetch" has been short by one. In "All sessions" the three session
   * listeners rewrite the derived set without touching the selection, and a generation keyed
   * on the selected session would miss exactly that; reloading the session list under a
   * single-session scope leaves this key alone and correctly changes nothing.
   */
  const fetchScopeKey = createMemo(() => `${scopeLimit()}|${scopeIds().join(",")}`);
  let paintedScopeKey: string | null = null;

  createEffect(() => {
    // Nothing is fetched until the mount has settled the scope: a fetch before that issues a
    // round of calls for a session the window is about to leave.
    if (!scopeSettled()) return;
    const key = fetchScopeKey();
    if (key === paintedScopeKey) return;
    paintedScopeKey = key;

    // A scope change drops the content it invalidates, SYNCHRONOUSLY. A commit-time check
    // stops a stale result from being written; it does not remove rows already on screen,
    // which would otherwise stay for the whole round trip and stay forever if the new fetch
    // fails. "A correct selector over stale rows" is the failure this exists to prevent.
    //
    // The drop is as wide as the key that triggered it: leaving one session for "All
    // sessions" narrows the per-session limit from 500 to 100, so dropping only what left the
    // scope would keep 500 rows of a session now entitled to 100.
    const ids = scopeIds();
    const limit = scopeLimit();
    setSnapshots([]);
    setRows((prev) => capPerSession(keepSessions(prev, ids), limit));
    setLoadError("");
    void refresh();
  });

  /**
   * One poll round, re-armed on every path.
   *
   * #1188: the re-arm used to live inside `refresh().then()`, so any outcome that was not a
   * fulfilment ended the chain for the life of the window, and the reported outcome was no
   * outcome at all. It is in `finally` now, and the fetch has a deadline (`withDeadline`),
   * because `finally` does not run for a promise that never settles either. The re-arm is
   * the FIRST statement in the `finally`: nothing may ever come between a round and the next
   * one, and `settingsStore.refresh()` is best-effort by construction.
   */
  const runPollRound = async () => {
    try {
      await refresh();
    } catch (err) {
      // `refresh` swallows its own failures, so this is unreachable today. It is the belt for
      // anything later added outside its try: an escape here would end the chain again.
      console.error("[watchers] poll round failed:", err);
    } finally {
      if (!disposed) {
        schedulePoll();
        // The poll also refreshes the settings store, so a watcher saved from the modal turns
        // the "no watcher reaches this agent" state into "configured and waiting" without the
        // user reopening the window. There is no cross-window settings event to use instead.
        settingsStore.refresh();
      }
    }
  };

  const schedulePoll = () => {
    if (disposed) return;
    if (pollTimer) clearTimeout(pollTimer);
    const delay = document.hasFocus() ? POLL_FOCUSED_MS : POLL_UNFOCUSED_MS;
    pollTimer = setTimeout(() => {
      pollTimer = null;
      // Chained rather than fired: a round that outlives its own period would otherwise stack
      // against a per-session mutex, and in "All sessions" one round is already N calls.
      void runPollRound();
    }, delay);
  };

  onMount(async () => {
    try {
      // The store does not autoload, and both the live agent-label fallback and the theme
      // need it. A failure here costs labels and the theme, not the window.
      await settingsStore
        .load()
        .catch((err) => console.error("[watchers] settings load:", err));
      if (disposed) return;

      // Subscribe BEFORE fetching: a match landing between the two would otherwise be lost.
      // The overlap it creates is exact rather than heuristic, because the merge keys on
      // `(sessionId, seq)`.
      await register(
        onWatcherMatches((batch) => {
          if (!scopeIds().includes(batch.sessionId)) return;
          const session = sessionById(batch.sessionId);
          const incoming = batch.matches.map((match) => freezeRow(match, session));
          setRows((prev) => capPerSession(mergeRows(prev, incoming), scopeLimit()));
          if (pinnedTop() && scrollEl) scrollEl.scrollTop = 0;
        })
      );
      await register(
        onWatchersScopeRequest(({ sessionId }) => {
          scopeEventGeneration += 1;
          setScopeSessionId(sessionId);
          // No fetch is issued here. Every fetch is issued by the scope effect, keyed on the
          // fetch scope itself.
        })
      );
      await register(onSessionCreated(() => void reloadSessions()));
      await register(onSessionDestroyed(() => void reloadSessions()));
      await register(onSessionRenamed(() => void reloadSessions()));

      await reloadSessions();
      if (disposed) return;

      // The pull, issued AFTER the subscribe and adopted unless an event has been handled
      // since. The window label exists the moment the builder returns while this listener
      // exists only now, and Tauri queues nothing for a listener that did not exist yet, so an
      // emit that raced the subscribe is recovered exactly here. A failure leaves the query
      // parameter standing, which is the scope the window was opened with.
      const generationAtIssue = scopeEventGeneration;
      try {
        const pulled = await WindowAPI.getWatchersScope();
        if (disposed) return;
        if (pulled && scopeEventGeneration === generationAtIssue) setScopeSessionId(pulled);
      } catch (err) {
        if (disposed) return;
        console.error("[watchers] failed to pull the requested scope:", err);
      }

      // Only now may anything be fetched.
      setScopeSettled(true);
      schedulePoll();

      if (isTauri) {
        // Held in a variable that the SYNCHRONOUS `onCleanup` below runs. Calling `onCleanup`
        // from an async continuation would attach to no owner at all, and these listeners
        // would outlive the window.
        cleanupGeometry = await trackGeometry();
        if (disposed) {
          cleanupGeometry?.();
          cleanupGeometry = null;
        }
      }
    } catch (error) {
      // Never re-thrown. Solid does not await the promise this `onMount` returns, so anything
      // that escapes here is an unhandled rejection instead of an error somebody sees, and
      // the window would sit half-started with nothing on screen saying so. The sentinel is
      // not a failure at all: it means the window closed while the mount was still running.
      if (error === mountDisposed || disposed) return;
      console.error("[watchers] the window failed to finish starting up:", error);
      setLoadError(error instanceof Error ? error.message : String(error));
    }
  });

  /** Persist through the dedicated one-field command, never `initWindowGeometry`: that one
   *  read-modify-writes the whole AppSettings on a debounce, which races the Settings save
   *  that edits the watcher map. The rect is converted to logical pixels first; see
   *  `logicalGeometry`. */
  const trackGeometry = async (): Promise<(() => void) | null> => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    if (disposed) return null;
    const win = getCurrentWindow();
    let saveTimer: ReturnType<typeof setTimeout> | null = null;

    const save = () => {
      if (saveTimer) clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        void (async () => {
          try {
            const factor = await win.scaleFactor();
            const position = await win.outerPosition();
            // `innerSize`, because the builder restores through `inner_size`.
            const size = await win.innerSize();
            if (disposed) return;
            await WindowAPI.setWatchersGeometry(
              logicalGeometry(
                { x: position.x, y: position.y, width: size.width, height: size.height },
                factor
              )
            );
          } catch (err) {
            console.error("[watchers] failed to save geometry:", err);
          }
        })();
      }, GEOMETRY_SAVE_DEBOUNCE_MS);
    };

    const stopSaving = () => {
      if (saveTimer) clearTimeout(saveTimer);
      saveTimer = null;
    };

    try {
      const release = await registerAll(
        [() => win.onMoved(save), () => win.onResized(save)],
        () => disposed
      );
      if (!release) {
        stopSaving();
        return null;
      }
      return () => {
        release();
        stopSaving();
      };
    } catch (err) {
      // Geometry tracking is best-effort: losing it costs a persisted rect, not the window.
      // What must not happen is escaping into `onMount`, whose promise nobody awaits.
      stopSaving();
      console.error("[watchers] failed to track the window geometry:", err);
      return null;
    }
  };

  onCleanup(() => {
    disposed = true;
    for (const unlisten of listeners) unlisten();
    listeners.length = 0;
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
    cleanupGeometry?.();
    cleanupGeometry = null;
  });

  const onScroll = () => {
    if (!scrollEl) return;
    setPinnedTop(scrollEl.scrollTop <= PIN_TO_TOP_SLACK_PX);
  };

  const changeScope = (value: string) => {
    // No fetch here either: changing the scope changes `fetchScopeKey`, and that is what
    // invalidates, drops and refetches.
    setScopeSessionId(value === "all" ? null : value);
  };

  const openWatcherSettings = () => {
    // Focus first: the Settings modal is mounted only in the sidebar, so with this window in
    // front the modal would open behind it and nothing visible would happen.
    WindowAPI.focusMain()
      .catch((err) => console.error("[watchers] focus main:", err))
      .finally(() => {
        emitOpenSettings("watchers").catch((err) =>
          console.error("[watchers] open settings:", err)
        );
      });
  };

  return (
    <div class="watchers-window" data-ac-testid="watchers.window">
      <WatchersTitlebar />

      <div class="watchers-filter-bar" data-ac-testid="watchers.filters" data-ac-role="toolbar">
        <div class="watchers-filter-group">
          <span class="watchers-filter-label">Agent</span>
          <select
            class="watchers-select"
            ref={(el) => (scopeEl = el)}
            onChange={(e) => changeScope(e.currentTarget.value)}
            data-ac-testid="watchers.scope"
            data-ac-role="combobox"
          >
            <option value="all">All agents</option>
            <For each={agentSessions()}>
              {(session) => <option value={session.id}>{session.name}</option>}
            </For>
          </select>
        </div>

        <Show when={watcherOptions().length > 0}>
          <div class="watchers-filter-group" data-ac-testid="watchers.filter.watcher">
            <span class="watchers-filter-label">Watcher</span>
            <For each={watcherOptions()}>
              {(value) => (
                <button
                  type="button"
                  class="watchers-chip"
                  classList={{ "is-active": watcherFilter().has(value) }}
                  onClick={() => toggleFilter(watcherFilter, setWatcherFilter, value)}
                  aria-pressed={watcherFilter().has(value)}
                  data-ac-testid={`watchers.filter.watcher.${value}`}
                  data-ac-role="button"
                >
                  {value}
                </button>
              )}
            </For>
          </div>
        </Show>

        {/* In single-session scope Coding-Agent and Workgroup have one possible value each and
            filter nothing, so they are not rendered -- the gesture the Resource Monitor
            already uses for an empty option list. */}
        <Show when={isAllSessions() && agentOptions().length > 0}>
          <div class="watchers-filter-group" data-ac-testid="watchers.filter.agent">
            <span class="watchers-filter-label">Coding-Agent</span>
            <For each={agentOptions()}>
              {(option) => (
                <button
                  type="button"
                  class="watchers-chip"
                  classList={{ "is-active": agentFilter().has(option.id) }}
                  onClick={() => toggleFilter(agentFilter, setAgentFilter, option.id)}
                  aria-pressed={agentFilter().has(option.id)}
                  data-ac-testid={`watchers.filter.agent.${option.id}`}
                  data-ac-role="button"
                >
                  {agentChipText(option)}
                </button>
              )}
            </For>
          </div>
        </Show>

        <Show when={isAllSessions() && workgroupOptions().length > 0}>
          <div class="watchers-filter-group" data-ac-testid="watchers.filter.workgroup">
            <span class="watchers-filter-label">Workgroup</span>
            <For each={workgroupOptions()}>
              {(value) => (
                <button
                  type="button"
                  class="watchers-chip"
                  classList={{ "is-active": workgroupFilter().has(value) }}
                  onClick={() => toggleFilter(workgroupFilter, setWorkgroupFilter, value)}
                  aria-pressed={workgroupFilter().has(value)}
                  data-ac-testid={`watchers.filter.workgroup.${value}`}
                  data-ac-role="button"
                >
                  {value}
                </button>
              )}
            </For>
          </div>
        </Show>

        <input
          class="watchers-search"
          type="text"
          placeholder="Filter captures..."
          value={textFilter()}
          onInput={(e) => setTextFilter(e.currentTarget.value)}
          data-ac-testid="watchers.filter.text"
          data-ac-role="searchbox"
        />

        <Show when={filtersActive()}>
          <button
            type="button"
            class="watchers-clear"
            onClick={clearFilters}
            data-ac-testid="watchers.filter.clear"
            data-ac-role="button"
          >
            Clear filters
          </button>
        </Show>
      </div>

      <Show when={loadError()}>
        <div class="watchers-banner watchers-banner-error" data-ac-testid="watchers.error">
          {loadError()}
        </div>
      </Show>

      {/* Two distinct signals, never merged: `truncated` is exact knowledge that something
          was dropped; `possiblyMissedFrames` is uncertainty about whether anything was. */}
      <Show when={truncated()}>
        <div class="watchers-banner" data-ac-testid="watchers.truncated">
          Older activations were dropped (buffer limit)
        </div>
      </Show>
      <Show when={missedFrames() > 0}>
        <div class="watchers-note" data-ac-testid="watchers.missedFrames">
          Some screen output was not sampled
        </div>
      </Show>
      {/* Outside the table states on purpose. A degraded watcher has by definition already
          emitted matches, so a marker living inside the "configured and waiting" branch is
          exactly where it can never be reached: one visible row sends the window to the
          table instead. */}
      <Show when={degraded().length > 0}>
        <div class="watchers-note watchers-note-degraded" data-ac-testid="watchers.degraded">
          {`Degraded: ${degraded()
            .map((counter) => counter.watcherId)
            .join(", ")} hit a per-tick cap, so some activations were not recorded.`}
        </div>
      </Show>

      <div
        class="watchers-body"
        ref={(el) => (scrollEl = el)}
        onScroll={onScroll}
        data-ac-testid="watchers.body"
      >
        <Show when={view() === "warming"}>
          <div class="watchers-empty" data-ac-testid="watchers.empty.warming" data-ac-role="status">
            Waiting for the first sample...
          </div>
        </Show>

        <Show when={view() === "unconfigured"}>
          <div
            class="watchers-empty"
            data-ac-testid="watchers.empty.unconfigured"
            data-ac-role="status"
          >
            <p>No configured watcher reaches this session's agent.</p>
            <button
              type="button"
              class="watchers-cta"
              onClick={openWatcherSettings}
              data-ac-testid="watchers.configure"
              data-ac-role="button"
            >
              Configure watchers
            </button>
          </div>
        </Show>

        <Show when={view() === "waiting"}>
          <div
            class="watchers-empty"
            data-ac-testid="watchers.empty.waiting"
            data-ac-role="status"
          >
            <p>Configured and waiting. Nothing has matched yet.</p>
            <ul class="watchers-waiting-list">
              <For each={activeWatchers()}>
                {(counter) => (
                  <li data-ac-testid={`watchers.waiting.${counter.watcherId}`}>
                    <span class="watchers-chip is-static">{counter.watcherId}</span>
                    <span class="watchers-mode">{counter.mode}</span>
                    <span class="watchers-count">{counter.count}</span>
                    <Show when={counter.degraded}>
                      <span class="watchers-degraded">degraded</span>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>

        {/* Its own state, because saying "nothing has matched yet" over activations the user
            is filtering out is false and gives them nothing to act on. */}
        <Show when={view() === "filtered"}>
          <div
            class="watchers-empty"
            data-ac-testid="watchers.empty.filtered"
            data-ac-role="status"
          >
            <p>{`No activations match the current filters (${rows().length} hidden).`}</p>
            <button
              type="button"
              class="watchers-cta"
              onClick={clearFilters}
              data-ac-testid="watchers.filtered.clear"
              data-ac-role="button"
            >
              Clear filters
            </button>
          </div>
        </Show>

        <Show when={view() === "rows"}>
          <table class="watchers-table" data-ac-testid="watchers.table">
            <thead>
              <tr>
                <th class="watchers-col-time">Time</th>
                <th class="watchers-col-watcher">Watcher</th>
                <Show when={isAllSessions()}>
                  <th class="watchers-col-session">Session</th>
                </Show>
                <th class="watchers-col-captures">Captures</th>
              </tr>
            </thead>
            <tbody>
              <For each={visibleRows()}>
                {(row) => {
                  const key = rowKey(row);
                  const expanded = () => expandedKey() === key;
                  const primary = () => row.captures[0] ?? row.row;
                  return (
                    <>
                      <tr
                        class="watchers-row"
                        classList={{ "is-expanded": expanded() }}
                        onClick={() => setExpandedKey(expanded() ? null : key)}
                        data-ac-testid={`watchers.row.${key}`}
                        data-ac-role="row"
                        data-ac-mode={row.mode}
                      >
                        <td class="watchers-col-time">{formatClockTime(row.at)}</td>
                        <td class="watchers-col-watcher">
                          <span
                            class="watchers-chip is-static"
                            style={{ "--watcher-hue": `${watcherHue(row.watcherId)}deg` }}
                          >
                            {row.watcherId}
                          </span>
                          {/* State and occurrence rows are otherwise identical, and the word
                              "state" invites reading the last such row as the current state. */}
                          <span class="watchers-mode" title={modeTitle(row.mode)}>
                            {row.mode === "state" ? "state" : "occ"}
                          </span>
                        </td>
                        <Show when={isAllSessions()}>
                          <td class="watchers-col-session">
                            <div class="watchers-session-name">{row.sessionName}</div>
                            <div class="watchers-session-meta">
                              {[agentLabel(row), row.workgroup].filter(Boolean).join(" - ")}
                            </div>
                          </td>
                        </Show>
                        <td class="watchers-col-captures">
                          {/* Left-side ellipsis, so a path's tail (the file name) stays
                              visible, which is the part worth reading. */}
                          <span class="watchers-capture" dir="rtl">
                            {primary()}
                          </span>
                          <Show when={row.captures.length > 1}>
                            <For each={row.captures.slice(1)}>
                              {(capture) => (
                                <span class="watchers-capture-extra">{capture ?? "-"}</span>
                              )}
                            </For>
                          </Show>
                          <button
                            type="button"
                            class="watchers-copy"
                            title="Copy"
                            onClick={(e) => {
                              e.stopPropagation();
                              void navigator.clipboard?.writeText(primary());
                            }}
                            data-ac-testid={`watchers.copy.${key}`}
                            data-ac-role="button"
                          >
                            &#x29C9;
                          </button>
                        </td>
                      </tr>
                      <Show when={expanded()}>
                        <tr class="watchers-raw-row">
                          <td colSpan={isAllSessions() ? 4 : 3}>
                            {/* Wrapped, not scrolled: the real behavior of the repository's
                                raw-text container, and a nested horizontal scroller inside a
                                vertically scrolling table is worse for a 256-byte row. */}
                            <div class="watchers-raw" data-ac-testid={`watchers.raw.${key}`}>
                              {row.row}
                            </div>
                            <Show when={row.rowTruncated}>
                              <div class="watchers-note">Row truncated at 256 bytes</div>
                            </Show>
                          </td>
                        </tr>
                      </Show>
                    </>
                  );
                }}
              </For>
            </tbody>
          </table>
        </Show>
      </div>

      <div class="watchers-footer" data-ac-testid="watchers.footer">
        Best-effort. Activations can be missed. This is not an audit log.
      </div>
    </div>
  );
};

/** A stable hue per watcher id, so the same watcher keeps its colour across openings. */
function watcherHue(watcherId: string): number {
  let hash = 0;
  for (let i = 0; i < watcherId.length; i += 1) {
    hash = (hash * 31 + watcherId.charCodeAt(i)) % 360;
  }
  return hash;
}

function modeTitle(mode: string): string {
  return mode === "state"
    ? "State: records when the condition was first seen, not that it still holds"
    : "Occurrence: one event per match";
}

export default WatchersApp;
