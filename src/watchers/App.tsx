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
import type { Session, WatcherActivitySnapshot } from "../shared/types";
import {
  ALL_SESSIONS_LIMIT,
  SINGLE_SESSION_LIMIT,
  type ActivityRow,
  anyTruncated,
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
const POLL_FOCUSED_MS = 10_000;
const POLL_UNFOCUSED_MS = 15_000;

/** How close to the top still counts as pinned, in pixels. */
const PIN_TO_TOP_SLACK_PX = 8;

const WatchersApp: Component<{ initialSessionId?: string }> = (props) => {
  // `null` is the "All sessions" scope.
  const [scopeSessionId, setScopeSessionId] = createSignal<string | null>(
    props.initialSessionId ?? null
  );
  const [sessions, setSessions] = createSignal<Session[]>([]);
  const [rows, setRows] = createSignal<ActivityRow[]>([]);
  const [snapshots, setSnapshots] = createSignal<WatcherActivitySnapshot[]>([]);
  const [loadError, setLoadError] = createSignal("");

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

  /** Live agent label by id, falling back to what was frozen into the row, so renaming an
   *  entry updates old rows instead of leaving them lying. Precedent: `liveAgentLabel`. */
  const agentLabel = (row: ActivityRow): string => {
    const live = row.agentId
      ? settingsStore.current?.agents?.find((a) => a.id === row.agentId)?.label
      : null;
    return live || row.frozenAgentLabel || row.agentId || "";
  };

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

  const view = createMemo(() => resolveView(snapshots(), visibleRows().length));
  const activeWatchers = createMemo(() => mergeActiveWatchers(snapshots()));
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

  const refresh = async () => {
    const ids = scopeIds();
    const limit = isAllSessions() ? ALL_SESSIONS_LIMIT : SINGLE_SESSION_LIMIT;
    try {
      const fetched = await Promise.all(
        ids.map((id) => PtyAPI.getWatcherActivity(id, limit))
      );
      setSnapshots(fetched);
      setLoadError("");
      const incoming = fetched.flatMap((snapshot, index) =>
        snapshot.matches.map((match) => freezeRow(match, sessionById(ids[index])))
      );
      setRows((prev) => mergeRows(keepSessions(prev, ids), incoming));
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
    }
  };

  const reloadSessions = async () => {
    try {
      setSessions(await SessionAPI.list());
    } catch (err) {
      console.error("[watchers] failed to list sessions:", err);
    }
  };

  const listeners: UnlistenFn[] = [];
  let pollTimer: ReturnType<typeof setTimeout> | null = null;

  const schedulePoll = () => {
    if (pollTimer) clearTimeout(pollTimer);
    const delay = document.hasFocus() ? POLL_FOCUSED_MS : POLL_UNFOCUSED_MS;
    pollTimer = setTimeout(() => {
      void refresh();
      // The poll also refreshes the settings store, so a watcher saved from the modal turns
      // the "no watcher reaches this agent" state into "configured and waiting" without the
      // user reopening the window. There is no cross-window settings event to use instead.
      settingsStore.refresh();
      schedulePoll();
    }, delay);
  };

  onMount(async () => {
    // The store does not autoload, and the live agent-label fallback needs it.
    settingsStore.load().catch((err) => console.error("[watchers] settings load:", err));

    // Subscribe BEFORE fetching: a match landing between the two would otherwise be lost.
    // The overlap it creates is exact rather than heuristic, because the merge keys on
    // `(sessionId, seq)`.
    listeners.push(
      await onWatcherMatches((batch) => {
        if (!scopeIds().includes(batch.sessionId)) return;
        const session = sessionById(batch.sessionId);
        const incoming = batch.matches.map((match) => freezeRow(match, session));
        setRows((prev) => mergeRows(prev, incoming));
        if (pinnedTop() && scrollEl) scrollEl.scrollTop = 0;
      })
    );
    listeners.push(
      await onWatchersScopeRequest(({ sessionId }) => {
        setScopeSessionId(sessionId);
        void refresh();
      })
    );
    listeners.push(
      await onSessionCreated(() => {
        void reloadSessions();
      })
    );
    listeners.push(
      await onSessionDestroyed(() => {
        void reloadSessions();
      })
    );
    listeners.push(
      await onSessionRenamed(() => {
        void reloadSessions();
      })
    );

    await reloadSessions();
    await refresh();
    schedulePoll();

    if (isTauri) void trackGeometry();
  });

  /** Persist through the dedicated one-field command, never `initWindowGeometry`: that one
   *  read-modify-writes the whole AppSettings on a debounce, which races the Settings save
   *  that edits the watcher map. */
  const trackGeometry = async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    let saveTimer: ReturnType<typeof setTimeout> | null = null;
    const save = () => {
      if (saveTimer) clearTimeout(saveTimer);
      saveTimer = setTimeout(async () => {
        try {
          const position = await win.outerPosition();
          const size = await win.outerSize();
          await WindowAPI.setWatchersGeometry({
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
          });
        } catch (err) {
          console.error("[watchers] failed to save geometry:", err);
        }
      }, 500);
    };
    const unlistenMoved = await win.onMoved(save);
    const unlistenResized = await win.onResized(save);
    onCleanup(() => {
      unlistenMoved();
      unlistenResized();
      if (saveTimer) clearTimeout(saveTimer);
    });
  };

  onCleanup(() => {
    for (const unlisten of listeners) unlisten();
    listeners.length = 0;
    if (pollTimer) clearTimeout(pollTimer);
  });

  const onScroll = () => {
    if (!scrollEl) return;
    setPinnedTop(scrollEl.scrollTop <= PIN_TO_TOP_SLACK_PX);
  };

  const changeScope = (value: string) => {
    setScopeSessionId(value === "all" ? null : value);
    void refresh();
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
          <span class="watchers-filter-label">Scope</span>
          <select
            class="watchers-select"
            ref={(el) => (scopeEl = el)}
            onChange={(e) => changeScope(e.currentTarget.value)}
            data-ac-testid="watchers.scope"
            data-ac-role="combobox"
          >
            <option value="all">All sessions</option>
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

        {/* In single-session scope Agent and Workgroup have one possible value each and
            filter nothing, so they are not rendered -- the gesture the Resource Monitor
            already uses for an empty option list. */}
        <Show when={isAllSessions() && agentOptions().length > 0}>
          <div class="watchers-filter-group" data-ac-testid="watchers.filter.agent">
            <span class="watchers-filter-label">Agent</span>
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
