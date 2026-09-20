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
import iconUrl from "../assets/icon-16.png";
import { FILTER_DEBOUNCE_MS, MAX_PIDS, parsePidFilter } from "../shared/pid-filter";
import {
  ResourceMonitorAPI,
  SettingsAPI,
  WindowAPI,
  emitOpenSettings,
  emitResourceMonitorAttach,
} from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { centralViewStore } from "../main/stores/centralView";
import { resourceMonitorStore } from "../shared/stores/resourceMonitor";
import type {
  ResourceAgentGroupSnapshot,
  ResourceGroupState,
  ResourceOverallState,
  ResourceProcessSnapshot,
} from "../shared/types";
import "./styles/resource-monitor.css";

const DEFAULT_RESOURCE_PREFERENCES = {
  resourceBackoffPolling: true,
  resourceKeepLastSnapshot: true,
};

const NON_KILLABLE_GROUP_STATES = new Set<ResourceGroupState>([
  "terminating",
  "terminated",
  "failedCleanup",
  "unknownOwnership",
]);

const SECURITY_BLOCK_HINT =
  "The OS or security software is blocking process termination. Add an exclusion for AgentsCommander and the agent binaries.";

const formatBytes = (value?: number | null): string => {
  if (typeof value !== "number" || !Number.isFinite(value)) return "Unknown";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let next = value;
  let index = 0;
  while (Math.abs(next) >= 1024 && index < units.length - 1) {
    next /= 1024;
    index += 1;
  }
  const digits = index === 0 || next >= 100 ? 0 : 1;
  return `${next.toFixed(digits)} ${units[index]}`;
};

const formatCpu = (value?: number | null): string =>
  typeof value === "number" && Number.isFinite(value)
    ? `${value.toFixed(1)}%`
    : "Unknown";

const formatTimestamp = (value?: string | null): string => {
  if (!value) return "Never";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return parsed.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
};

const overallLabel = (state: ResourceOverallState): string => {
  switch (state) {
    case "ok":
      return "OK";
    case "warn":
      return "Warn";
    case "critical":
      return "Critical";
    case "enforcing":
      return "Enforcing";
    case "unknown":
      return "Unknown";
  }
};

const processName = (process: ResourceProcessSnapshot): string =>
  process.name || process.exeName || `pid ${process.pid}`;

const groupOrigin = (group: ResourceAgentGroupSnapshot): string =>
  `${group.workgroup ?? "-"} / ${group.agent ?? group.name}`;

const canKillGroup = (group: ResourceAgentGroupSnapshot): boolean =>
  group.killAllowed !== false && !NON_KILLABLE_GROUP_STATES.has(group.state);

const killActionLabel = (group: ResourceAgentGroupSnapshot): string =>
  group.state === "quarantined" ? "Force-kill" : "Kill";

const groupSeverity = (group: ResourceAgentGroupSnapshot): string => {
  if (group.state === "quarantined" || group.state === "failedCleanup") {
    return "critical";
  }
  if (group.state === "terminating") return "enforcing";
  if (group.state === "unknownOwnership" || group.lastError) return "warn";
  if (group.networkState === "unknown") return "unknown";
  return "ok";
};

const isActiveGroup = (group: ResourceAgentGroupSnapshot): boolean =>
  group.state !== "terminated";

type RmStatusFilter = "all" | "active" | "inactive";

const STATUS_FILTERS: ReadonlyArray<{ value: RmStatusFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "active", label: "Active" },
  { value: "inactive", label: "Inactive" },
];

const distinct = (values: (string | null | undefined)[]): string[] =>
  [...new Set(values.filter((v): v is string => !!v))].sort((a, b) => Number(a > b) - Number(a < b));

const toggleFilter = (
  get: () => Set<string>,
  set: (next: Set<string>) => void,
  value: string,
): void => {
  const next = new Set(get());
  if (next.has(value)) {
    next.delete(value);
  } else {
    next.add(value);
  }
  set(next);
};

// #2245 - sort, matching and debouncing for the integral view.

type RmSortField = "default" | "cpu" | "private" | "processes" | "name";
type RmSortDirection = "asc" | "desc";

const SORT_OPTIONS: ReadonlyArray<{ value: RmSortField; label: string }> = [
  { value: "default", label: "Default" },
  { value: "cpu", label: "CPU" },
  { value: "private", label: "Private" },
  { value: "processes", label: "Processes" },
  { value: "name", label: "Name" },
];

/** Descending for the three metrics, ascending for Name. Picked when the field
 *  changes so "sort by CPU" opens on the biggest consumer. */
const defaultDirectionFor = (field: RmSortField): RmSortDirection =>
  field === "name" ? "asc" : "desc";

const sortMetric = (
  group: ResourceAgentGroupSnapshot,
  field: RmSortField
): number | null | undefined => {
  if (field === "cpu") return group.cpuPercent;
  if (field === "private") return group.privateBytes;
  if (field === "processes") return group.processCount;
  return null;
};

/** Ties break by sessionId ascending in BOTH directions. That, and only that,
 *  is what makes the rendered order deterministic across polls. */
const bySessionId = (
  a: ResourceAgentGroupSnapshot,
  b: ResourceAgentGroupSnapshot
): number => Number(a.sessionId > b.sessionId) - Number(a.sessionId < b.sessionId);

const compareGroups = (
  a: ResourceAgentGroupSnapshot,
  b: ResourceAgentGroupSnapshot,
  field: RmSortField,
  direction: RmSortDirection
): number => {
  const flip = direction === "asc" ? 1 : -1;

  if (field === "name") {
    const base = Number(a.name > b.name) - Number(a.name < b.name);
    return base === 0 ? bySessionId(a, b) : base * flip;
  }

  const av = sortMetric(a, field);
  const bv = sortMetric(b, field);
  const aUnknown = typeof av !== "number" || !Number.isFinite(av);
  const bUnknown = typeof bv !== "number" || !Number.isFinite(bv);

  // Unknown sorts last in both directions: an "Unknown" must never head the
  // list of biggest consumers, and reversing must not promote it either.
  if (aUnknown && bUnknown) return bySessionId(a, b);
  if (aUnknown) return 1;
  if (bUnknown) return -1;
  if (av === bv) return bySessionId(a, b);
  return (av < bv ? -1 : 1) * flip;
};

const groupMatchesPids = (
  group: ResourceAgentGroupSnapshot,
  pids: ReadonlySet<number>
): boolean =>
  (typeof group.rootPid === "number" && pids.has(group.rootPid)) ||
  group.processes.some((process) => pids.has(process.pid));

const groupMatchesSearch = (
  group: ResourceAgentGroupSnapshot,
  needle: string
): boolean => {
  const fields = [group.name, group.agent, group.workgroup, group.project];
  if (fields.some((value) => !!value && value.toLowerCase().includes(needle))) {
    return true;
  }
  return group.processes.some((process) =>
    (process.name ?? process.exeName ?? "").toLowerCase().includes(needle)
  );
};

interface DebouncedApply {
  schedule: (value: string) => void;
  flush: (value: string) => void;
  cancel: () => void;
}

/**
 * Mirrors a typed value into an applied signal after FILTER_DEBOUNCE_MS.
 *
 * It holds exactly one handle and there is exactly ONE `clearTimeout` site, so
 * "dropped the per-keystroke cancel" is a single-line mutant. `flush` is what
 * the clear buttons call: it cancels the pending write AND applies the new
 * value synchronously, so a timeout armed a moment earlier can never resurrect
 * text the user just cleared.
 */
const debouncedApply = (setApplied: (value: string) => void): DebouncedApply => {
  let handle: ReturnType<typeof setTimeout> | null = null;

  const clear = (): void => {
    if (handle === null) return;
    clearTimeout(handle);
    handle = null;
  };

  return {
    schedule(value) {
      clear();
      handle = setTimeout(() => {
        handle = null;
        setApplied(value);
      }, FILTER_DEBOUNCE_MS);
    },
    flush(value) {
      clear();
      setApplied(value);
    },
    cancel: clear,
  };
};

const PID_HELP_TEXT = "Comma-separated PIDs from the observed process trees.";
const PID_UNMATCHED_TITLE = "Not present in the observed processes of this snapshot";
const PARTIAL_PILL_TITLE = "Not all descendants were observed in this snapshot";
const COVERAGE_NOTICE =
  "Some process trees are only partially observed; a PID may be missing from this snapshot.";
const MONITOR_DISABLED_TITLE =
  "Resource monitoring is disabled, so there is nothing to filter.";
const KILL_MODAL_TITLE_ID = "rm-kill-modal-title";

/** Indentation of a process row, computed in JS rather than with calc(): jsdom
 *  reports `calc(12px * 2)` back as "calc(24px)", so a calc() would make the
 *  test fail on the shape instead of on the behaviour. Clamped at 6 so a deep
 *  tree cannot push the name out of its column. */
const depthPadding = (depth?: number): string =>
  `${12 * Math.min(depth ?? 0, 6)}px`;

const processRowTitle = (process: ResourceProcessSnapshot): string =>
  typeof process.parentPid === "number"
    ? `PID ${process.pid} - parent ${process.parentPid}`
    : `PID ${process.pid}`;

/** The enabled, focusable elements inside the dialog. Computed as a list rather
 *  than special-cased on the two buttons: while a kill is in flight BOTH are
 *  disabled and focus sits on the container, and a two-case handler lets Tab
 *  and Shift+Tab escape the dialog from there. */
const focusablesIn = (root: HTMLElement): HTMLElement[] =>
  Array.from(
    root.querySelectorAll<HTMLElement>(
      "button, input, select, textarea, a[href], [tabindex]:not([tabindex='-1'])"
    )
  ).filter((el) => !el.hasAttribute("disabled"));

const Titlebar: Component = () => {
  const [maximized, setMaximized] = createSignal(false);

  const handleAttach = async () => {
    try {
      await emitResourceMonitorAttach();
      if (isTauri) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().close();
      }
    } catch (err) {
      console.error("Attach resource monitor failed:", err);
    }
  };

  const handleMinimize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().minimize();
  };

  const handleMaximize = async () => {
    if (!isTauri) return;
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      if (await win.isMaximized()) {
        await win.unmaximize();
      } else {
        await win.maximize();
      }
      setMaximized(await win.isMaximized());
    } catch (err) {
      console.error("Resource monitor toggle maximize failed:", err);
    }
  };

  const handleClose = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
  };

  onMount(() => {
    if (!isTauri) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    onCleanup(() => {
      disposed = true;
      unlisten?.();
    });
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        setMaximized(await win.isMaximized());
        const stop = await win.onResized(() => {
          void win.isMaximized().then(setMaximized).catch(() => {});
        });
        if (disposed) {
          stop();
          return;
        }
        unlisten = stop;
      } catch (err) {
        console.error("Resource monitor maximize-state tracking failed:", err);
      }
    })();
  });

  return (
    <div class="rm-titlebar" data-tauri-drag-region>
      <div class="rm-titlebar-brand" data-tauri-drag-region>
        <img src={iconUrl} class="rm-titlebar-icon" alt="" draggable={false} />
        <span class="rm-titlebar-title" data-tauri-drag-region>
          Resource Monitor
        </span>
      </div>
      <Show when={isTauri}>
        <div class="rm-titlebar-controls">
          <button
            class="rm-titlebar-btn"
            onClick={handleAttach}
            title="Attach"
            aria-label="Attach Resource Monitor to main window"
            data-ac-testid="resourceMonitor.titlebar.attach"
            data-ac-role="button"
          >
            <span
              class="rm-titlebar-btn-alias"
              data-ac-testid="resourceMonitor.attach"
              data-ac-role="button"
            >
              &#x21B2;
            </span>
          </button>
          <button
            class="rm-titlebar-btn"
            onClick={handleMinimize}
            title="Minimize"
            aria-label="Minimize Resource Monitor"
            data-ac-testid="resourceMonitor.titlebar.minimize"
            data-ac-role="button"
          >
            <span
              class="rm-titlebar-btn-alias"
              data-ac-testid="resourceMonitor.minimize"
              data-ac-role="button"
            >
              &#x2014;
            </span>
          </button>
          <button
            class="rm-titlebar-btn"
            onClick={handleMaximize}
            title={maximized() ? "Restore" : "Maximize"}
            aria-label={
              maximized() ? "Restore Resource Monitor" : "Maximize Resource Monitor"
            }
            data-ac-testid="resourceMonitor.titlebar.maximize"
            data-ac-role="button"
            data-ac-state={maximized() ? "maximized" : "normal"}
          >
            <span
              class="rm-titlebar-btn-alias"
              data-ac-testid="resourceMonitor.maximize"
              data-ac-role="button"
            >
              {/* U+2750 = restore glyph (shown when maximized), U+25A1 = maximize
                  glyph. Built from char codes so the source stays ASCII-only,
                  avoiding a literal glyph or \u escape that tooling can mangle. */}
              {maximized() ? String.fromCharCode(0x2750) : String.fromCharCode(0x25A1)}
            </span>
          </button>
          <button
            class="rm-titlebar-btn rm-titlebar-btn-close"
            onClick={handleClose}
            title="Close"
            aria-label="Close Resource Monitor"
            data-ac-testid="resourceMonitor.titlebar.close"
            data-ac-role="button"
          >
            <span
              class="rm-titlebar-btn-alias"
              data-ac-testid="resourceMonitor.close"
              data-ac-role="button"
            >
              &#x2715;
            </span>
          </button>
        </div>
      </Show>
    </div>
  );
};

interface ResourceMonitorAppProps {
  embedded?: boolean;
}

const ResourceMonitorApp: Component<ResourceMonitorAppProps> = (props) => {
  // #2245 - several groups can be open at once, so this is a Set rather than a
  // single id. Every write produces a NEW Set so Solid sees the change.
  const [expandedGroupIds, setExpandedGroupIds] = createSignal<Set<string>>(
    new Set()
  );
  const [killTarget, setKillTarget] =
    createSignal<ResourceAgentGroupSnapshot | null>(null);
  const [killError, setKillError] = createSignal("");
  const [killInFlight, setKillInFlight] = createSignal(false);
  const [killResult, setKillResult] = createSignal<{
    sessionId: string;
    state: ResourceGroupState;
    message: string;
    blockedBySecurity: boolean;
  } | null>(null);

  let modalRef: HTMLDivElement | undefined;
  let cancelRef: HTMLButtonElement | undefined;
  let groupsHeadingRef: HTMLHeadingElement | undefined;

  const openKillModal = (group: ResourceAgentGroupSnapshot) => {
    setKillError("");
    setKillResult(null);
    setKillTarget(group);
  };

  // <For> recreates every row on each poll, so a DOM reference stored at open
  // time can be detached by the time the modal closes. Re-query instead, and
  // fall back to the Agents heading when the group itself is gone. Never body.
  const restoreFocusAfterClose = (sessionId: string) => {
    const button = document.querySelector<HTMLElement>(
      `[data-ac-testid="resourceMonitor.group.${sessionId}.kill"]`
    );
    (button ?? groupsHeadingRef)?.focus();
  };

  /**
   * The single close path, used by Cancel, by Escape and by a finalized kill.
   *
   * `settle` is what the finalized path passes: the restore must run AFTER the
   * refresh, not before it. Restoring first lands focus on a Kill button that
   * the refresh is about to delete, and focus then falls to body.
   */
  const closeKillModal = async (
    sessionId: string,
    settle?: () => Promise<unknown>
  ): Promise<void> => {
    setKillTarget(null);
    setKillError("");
    if (settle) await settle();
    restoreFocusAfterClose(sessionId);
  };

  const isVerifying = (group: ResourceAgentGroupSnapshot): boolean =>
    group.state === "terminating" ||
    (killInFlight() && killTarget()?.sessionId === group.sessionId);

  const handleDetach = async () => {
    try {
      await WindowAPI.openResourceMonitor();
      centralViewStore.showTerminal();
    } catch (err) {
      console.error("Detach resource monitor failed:", err);
    }
  };

  onMount(async () => {
    let resourcePreferences = DEFAULT_RESOURCE_PREFERENCES;
    try {
      const settings = await SettingsAPI.get();
      resourcePreferences = {
        resourceBackoffPolling: settings.resourceBackoffPolling,
        resourceKeepLastSnapshot: settings.resourceKeepLastSnapshot,
      };
      if (!props.embedded) {
        document.documentElement.classList.toggle("light-theme", settings.themeLight);
      }
    } catch (err) {
      console.error("Failed to load resource-monitor settings:", err);
    }

    const stopPolling = resourceMonitorStore.startPolling({
      activeIntervalMs: 2_000,
      idleIntervalMs: 10_000,
      backoffIntervalMs: 15_000,
      backoffWhenIdle: resourcePreferences.resourceBackoffPolling,
      keepLastSnapshot: resourcePreferences.resourceKeepLastSnapshot,
    });
    void resourceMonitorStore.refresh();
    onCleanup(stopPolling);
  });

  const snapshot = () => resourceMonitorStore.snapshot;
  const groups = createMemo(() => snapshot()?.groups ?? []);

  const [statusFilter, setStatusFilter] = createSignal<RmStatusFilter>("all");
  const [projectFilter, setProjectFilter] = createSignal<Set<string>>(new Set());
  const [workgroupFilter, setWorkgroupFilter] = createSignal<Set<string>>(
    new Set()
  );
  const [roleFilter, setRoleFilter] = createSignal<Set<string>>(new Set());

  // #2245 - the typed text is visible immediately; only the applied value is
  // delayed, and only the applied value participates in filtering.
  const [pidText, setPidText] = createSignal("");
  const [appliedPidText, setAppliedPidText] = createSignal("");
  const [searchText, setSearchText] = createSignal("");
  const [appliedSearchText, setAppliedSearchText] = createSignal("");
  const pidDebounce = debouncedApply(setAppliedPidText);
  const searchDebounce = debouncedApply(setAppliedSearchText);
  onCleanup(() => {
    pidDebounce.cancel();
    searchDebounce.cancel();
  });

  const [sortField, setSortField] = createSignal<RmSortField>("default");
  const [sortDirection, setSortDirection] = createSignal<RmSortDirection>(
    defaultDirectionFor("default")
  );

  const pidParse = createMemo(() => parsePidFilter(appliedPidText()));
  const appliedPids = createMemo(() => new Set(pidParse().pids));
  const appliedSearch = createMemo(() => appliedSearchText().trim().toLowerCase());
  const monitorDisabled = createMemo(() => snapshot()?.monitorEnabled === false);

  const projectOptions = createMemo(() =>
    distinct(groups().map((g) => g.project))
  );
  const workgroupOptions = createMemo(() =>
    distinct(groups().map((g) => g.workgroup))
  );
  const roleOptions = createMemo(() => distinct(groups().map((g) => g.agent)));

  // Everything except the PID set. The coverage notice of 3.4 is evaluated over
  // THIS list, not over the visible one: the useful case is a PID missing
  // BECAUSE its tree is partial, and that group is exactly the one the PID
  // filter has just removed from view.
  const nonPidFilteredGroups = createMemo(() => {
    const status = statusFilter();
    const projects = projectFilter();
    const wgs = workgroupFilter();
    const roles = roleFilter();
    const needle = appliedSearch();
    return groups().filter((g) => {
      if (status === "active" && !isActiveGroup(g)) return false;
      if (status === "inactive" && isActiveGroup(g)) return false;
      if (projects.size > 0 && !(g.project && projects.has(g.project)))
        return false;
      if (wgs.size > 0 && !(g.workgroup && wgs.has(g.workgroup))) return false;
      if (roles.size > 0 && !(g.agent && roles.has(g.agent))) return false;
      if (needle.length > 0 && !groupMatchesSearch(g, needle)) return false;
      return true;
    });
  });

  const filteredGroups = createMemo(() => {
    const pids = appliedPids();
    if (pids.size === 0) return nonPidFilteredGroups();
    return nonPidFilteredGroups().filter((g) => groupMatchesPids(g, pids));
  });

  // Written and read ONLY inside the sortedGroups memo, and deliberately a
  // plain Map: touching it never schedules a recomputation, so there is no
  // feedback loop between "a row is pinned" and "the list re-sorts".
  const pinnedIndex = new Map<string, number>();
  let previousSortKey = "";
  let previousFilterKey = "";

  const filterSignature = createMemo(() =>
    JSON.stringify([
      statusFilter(),
      [...projectFilter()].sort(),
      [...workgroupFilter()].sort(),
      [...roleFilter()].sort(),
      pidParse().pids,
      appliedSearch(),
    ])
  );

  const sortedGroups = createMemo(() => {
    const field = sortField();
    const direction = sortDirection();
    const expanded = expandedGroupIds();

    // 1. Sort the filtered list.
    const sorted = [...filteredGroups()];
    if (field !== "default") {
      sorted.sort((a, b) => compareGroups(a, b, field, direction));
    }

    // 2. Reset on a sort or filter change. Step 5 then re-pins the open rows at
    //    their NEW positions in this same pass, so they do not jump next poll.
    const sortKey = `${field}|${direction}`;
    const filterKey = filterSignature();
    if (sortKey !== previousSortKey || filterKey !== previousFilterKey) {
      pinnedIndex.clear();
      previousSortKey = sortKey;
      previousFilterKey = filterKey;
    }

    // 3. Prune what is gone or no longer open. A pinned group that stops
    //    matching is dropped and is NOT reinserted: reinsertion only ever
    //    repositions a row already present in the sorted list.
    const present = new Set(sorted.map((g) => g.sessionId));
    for (const sessionId of [...pinnedIndex.keys()]) {
      if (!present.has(sessionId) || !expanded.has(sessionId)) {
        pinnedIndex.delete(sessionId);
      }
    }

    // 4. Reinsert the still-pinned rows at their recorded index, in ascending
    //    recorded-index order, each clamped to the current length. Skipped in
    //    Default, where the map is never consulted.
    let result = sorted;
    if (field !== "default" && pinnedIndex.size > 0) {
      const pins = [...pinnedIndex.entries()].sort((a, b) => a[1] - b[1]);
      const pinnedIds = new Set(pins.map(([sessionId]) => sessionId));
      const bySessionIdMap = new Map(sorted.map((g) => [g.sessionId, g]));
      result = sorted.filter((g) => !pinnedIds.has(g.sessionId));
      for (const [sessionId, index] of pins) {
        const group = bySessionIdMap.get(sessionId);
        if (group) result.splice(Math.min(index, result.length), 0, group);
      }
    }

    // 5. Register every open row that has no entry yet.
    result.forEach((group, index) => {
      if (expanded.has(group.sessionId) && !pinnedIndex.has(group.sessionId)) {
        pinnedIndex.set(group.sessionId, index);
      }
    });

    return result;
  });

  const filtersActive = createMemo(
    () =>
      statusFilter() !== "all" ||
      projectFilter().size > 0 ||
      workgroupFilter().size > 0 ||
      roleFilter().size > 0 ||
      appliedPids().size > 0 ||
      appliedSearch().length > 0
  );

  /** True when the PID set is the ONLY thing narrowing the list, which is what
   *  licenses the specific "no process matches PID ..." empty state. */
  const pidOnlyFilter = createMemo(
    () =>
      appliedPids().size > 0 &&
      statusFilter() === "all" &&
      projectFilter().size === 0 &&
      workgroupFilter().size === 0 &&
      roleFilter().size === 0 &&
      appliedSearch().length === 0
  );

  const clearFilters = () => {
    setStatusFilter("all");
    setProjectFilter(new Set<string>());
    setWorkgroupFilter(new Set<string>());
    setRoleFilter(new Set<string>());
    setPidText("");
    pidDebounce.flush("");
    setSearchText("");
    searchDebounce.flush("");
  };

  const clearPidFilter = () => {
    setPidText("");
    pidDebounce.flush("");
  };

  const clearSearchFilter = () => {
    setSearchText("");
    searchDebounce.flush("");
  };

  // The counter of 3.3: one element, always rendered, never gated on
  // filtersActive(). V and T are read from the group lists directly, so a clean
  // state reads "Showing 3 of 3 agents" rather than disappearing.
  const counterText = createMemo(() => {
    const visible = sortedGroups();
    let text = `Showing ${visible.length} of ${groups().length} agents`;

    const pids = appliedPids();
    if (pids.size === 0) return text;

    let matchingProcesses = 0;
    let rootOnlyGroups = 0;
    for (const group of visible) {
      let hits = 0;
      for (const process of group.processes) {
        if (pids.has(process.pid)) hits += 1;
      }
      matchingProcesses += hits;
      // A group matching BOTH ways counts as a process match and not as a
      // root-only one, which is what makes "matched by root PID only" true.
      if (
        hits === 0 &&
        typeof group.rootPid === "number" &&
        pids.has(group.rootPid)
      ) {
        rootOnlyGroups += 1;
      }
    }

    text += ` - ${matchingProcesses} matching process${
      matchingProcesses === 1 ? "" : "es"
    }`;
    if (rootOnlyGroups > 0) {
      text += `, ${rootOnlyGroups} matched by root PID only`;
    }
    return text;
  });

  /** Chip state is computed against the WHOLE snapshot, never against the
   *  visible list: otherwise a status filter plus the PID of an active group
   *  would dim the chip and claim the PID is absent, which is false. */
  const pidPresentInSnapshot = (pid: number): boolean =>
    groups().some((g) => groupMatchesPids(g, new Set([pid])));

  const pidProcessName = (pid: number): string | null => {
    for (const group of groups()) {
      for (const process of group.processes) {
        if (process.pid === pid) return processName(process);
      }
    }
    return null;
  };

  const pidNoticeText = createMemo(() => {
    const parsed = pidParse();
    const parts: string[] = [];
    if (parsed.rejected.length > 0) {
      parts.push(`Ignored invalid tokens: ${parsed.rejected.join(", ")}`);
    }
    if (parsed.truncated) {
      parts.push(`Only the first ${MAX_PIDS} PIDs are applied.`);
    }
    return parts.join(" ");
  });

  /** Error tone only when nothing at all could be applied. A typo in the fourth
   *  PID must not read as a failure of the whole filter. */
  const pidNoticeState = createMemo(() =>
    pidParse().rejected.length > 0 && pidParse().pids.length === 0
      ? "error"
      : "warn"
  );

  const coverageNoticeVisible = createMemo(
    () =>
      appliedPids().size > 0 &&
      nonPidFilteredGroups().some((g) => g.descendantsObserved === false)
  );

  const totalProcessCount = createMemo(() =>
    groups().reduce((sum, group) => sum + group.processCount, 0)
  );

  const emptyStateText = createMemo(() => {
    let text: string;
    if (resourceMonitorStore.loading && groups().length === 0) {
      text = "Loading snapshot...";
    } else if (groups().length === 0) {
      text = "No active agents";
    } else if (pidOnlyFilter()) {
      // The panel observes only this snapshot. It never says a PID does not
      // exist, only that it is absent from what was observed.
      text = `No process in this snapshot matches PID ${pidParse().pids.join(", ")}`;
    } else {
      text = "No agents match the filters";
    }
    if (resourceMonitorStore.stale && snapshot()) {
      text += ` Snapshot captured at ${formatTimestamp(snapshot()?.capturedAt)}`;
    }
    return text;
  });

  // Auto-expansion: when the APPLIED PID set changes, every group matching the
  // new set is added. Nothing is ever removed, so a manual collapse wins until
  // the applied set changes again, and clearing the filter collapses nothing.
  let previousAppliedPidKey = "";
  createEffect(() => {
    const pids = appliedPids();
    const observed = groups();
    const key = [...pids].join(",");
    if (key === previousAppliedPidKey) return;

    // The key is recorded only once there is actually a snapshot to match the
    // set against. A PID applied before the first snapshot lands would
    // otherwise burn the key against an empty group list, and the snapshot
    // arriving a moment later would find the key already seen and never
    // auto-expand anything.
    if (observed.length === 0) return;
    previousAppliedPidKey = key;
    if (pids.size === 0) return;

    const matching = observed.filter((g) => groupMatchesPids(g, pids));
    if (matching.length === 0) return;
    setExpandedGroupIds((current) => {
      const next = new Set(current);
      for (const group of matching) next.add(group.sessionId);
      return next;
    });
  });

  const statusClass = createMemo(() => {
    const s = snapshot();
    if (!s || s.overallState === "unknown") return "unknown";
    if (s.overallState === "ok" && s.networkState === "unknown") return "unknown";
    return s.overallState;
  });
  const statusText = createMemo(() =>
    overallLabel(snapshot()?.overallState ?? "unknown")
  );
  const isExpanded = (sessionId: string): boolean =>
    expandedGroupIds().has(sessionId);

  const toggleGroup = (sessionId: string) => {
    setExpandedGroupIds((current) => {
      const next = new Set(current);
      if (next.has(sessionId)) {
        next.delete(sessionId);
      } else {
        next.add(sessionId);
      }
      return next;
    });
  };

  const changeSortField = (value: RmSortField) => {
    setSortField(value);
    setSortDirection(defaultDirectionFor(value));
  };

  const toggleSortDirection = () => {
    setSortDirection((current) => (current === "asc" ? "desc" : "asc"));
  };

  // Both buttons are disabled while a kill settles, so the dialog holds no
  // enabled focusable element. Move focus to the container, which is why it
  // carries tabindex="-1".
  createEffect(() => {
    if (killInFlight() && killTarget()) modalRef?.focus();
  });

  const handleModalKeyDown = (event: KeyboardEvent, sessionId: string) => {
    if (event.key === "Escape") {
      if (killInFlight()) return;
      event.preventDefault();
      void closeKillModal(sessionId);
      return;
    }
    if (event.key !== "Tab") return;

    // Cyclic over a computed list, never two special cases on the two buttons:
    // from the container a two-case handler lets both keys leave the dialog.
    event.preventDefault();
    const items = modalRef ? focusablesIn(modalRef) : [];
    if (items.length === 0) {
      modalRef?.focus();
      return;
    }
    const active = document.activeElement as HTMLElement | null;
    const index = active ? items.indexOf(active) : -1;
    const next =
      index === -1
        ? event.shiftKey
          ? items.length - 1
          : 0
        : (index + (event.shiftKey ? -1 : 1) + items.length) % items.length;
    items[next].focus();
  };

  const openResourcesSettings = () => {
    emitOpenSettings("resources").catch((err) =>
      console.error("Failed to open Resources settings:", err)
    );
  };

  const confirmKill = async () => {
    const target = killTarget();
    if (!target || killInFlight()) return;

    setKillError("");
    setKillInFlight(true);
    try {
      const result = await ResourceMonitorAPI.killGroup({
        sessionId: target.sessionId,
        reason: "user",
      });
      if (result.finalized) {
        setKillResult(null);
        // The restore runs AFTER the refresh: the group this focus would return
        // to is exactly the one the refresh is about to remove.
        await closeKillModal(target.sessionId, () =>
          resourceMonitorStore.refresh()
        );
      } else {
        setKillResult({
          sessionId: result.sessionId,
          state: result.state,
          message: result.message,
          blockedBySecurity: result.blockedBySecurity,
        });
        await resourceMonitorStore.refresh();
      }
    } catch (err) {
      setKillError(err instanceof Error ? err.message : String(err));
    } finally {
      setKillInFlight(false);
    }
  };

  return (
    <div
      class="rm-root"
      data-ac-testid="resourceMonitor.window"
      data-ac-role="surface"
    >
      <Show when={!props.embedded}>
        <Titlebar />
      </Show>
      <main class="rm-body">
        <header class="rm-header">
          <div>
            <div class="rm-eyebrow">AgentsCommander</div>
            <h1>Resource Monitor</h1>
          </div>
          <div class="rm-header-actions">
            <Show when={props.embedded}>
              <button
                class="rm-action-btn"
                onClick={handleDetach}
                title="Detach to a separate window"
                data-ac-testid="resourceMonitor.detach"
                data-ac-role="button"
              >
                Detach
              </button>
            </Show>
            <button
              class="rm-action-btn"
              onClick={() => resourceMonitorStore.refresh()}
              disabled={resourceMonitorStore.loading}
              data-ac-testid="resourceMonitor.refresh"
              data-ac-role="button"
            >
              Refresh
            </button>
            <button
              class="rm-action-btn"
              onClick={openResourcesSettings}
              data-ac-testid="resourceMonitor.settings"
              data-ac-role="button"
            >
              Settings
            </button>
          </div>
        </header>

        <section
          class="rm-status-strip"
          aria-label="Resource summary"
          data-ac-testid="resourceMonitor.summary"
          data-ac-role="status"
        >
          <div
            class={`rm-status-tile state-${statusClass()}`}
            data-ac-testid="resourceMonitor.summary.state"
            data-ac-role="metric"
            data-ac-state={statusClass()}
          >
            <span class="rm-tile-label">State</span>
            <strong>{statusText()}</strong>
          </div>
          <div
            class="rm-status-tile"
            data-ac-testid="resourceMonitor.summary.activeGroups"
            data-ac-role="metric"
          >
            <span class="rm-tile-label">Active Agents</span>
            <strong>
              <span
                data-ac-testid="resourceMonitor.summary.activeGroups.count"
                data-ac-role="metric"
              >
                {snapshot()?.activeAgentGroups ?? "Unknown"}
              </span>
              {" / "}
              <span
                data-ac-testid="resourceMonitor.summary.activeGroups.limit"
                data-ac-role="metric"
              >
                {snapshot()?.maxConcurrentAgentGroups ?? "Unknown"}
              </span>
            </strong>
          </div>
          <div
            class="rm-status-tile"
            data-ac-testid="resourceMonitor.summary.processCount"
            data-ac-role="metric"
          >
            <span class="rm-tile-label">Processes</span>
            {/* Over ALL groups in the snapshot, not the filtered ones: no other
                tile reacts to filtering either. */}
            <strong>{totalProcessCount()}</strong>
          </div>
          <div
            class="rm-status-tile"
            data-ac-testid="resourceMonitor.summary.appPrivateBytes"
            data-ac-role="metric"
          >
            <span class="rm-tile-label">App Private</span>
            <strong>{formatBytes(snapshot()?.appPrivateBytes)}</strong>
            {/* The ONE remaining visually hidden metric. aria-hidden so the
                bare number stops being announced; the testid is untouched. */}
            <span
              class="rm-automation-metric"
              aria-hidden="true"
              data-ac-testid="resourceMonitor.summary.appWorkingSetBytes"
              data-ac-role="metric"
            >
              {formatBytes(snapshot()?.appWorkingSetBytes)}
            </span>
          </div>
          <div
            class={`rm-status-tile network-${snapshot()?.networkState ?? "unknown"}`}
            data-ac-testid="resourceMonitor.summary.network"
            data-ac-role="metric"
            data-ac-state={snapshot()?.networkState ?? "unknown"}
          >
            <span class="rm-tile-label">Network</span>
            <strong>{snapshot()?.networkSummary ?? "Unknown"}</strong>
          </div>
        </section>

        <Show when={monitorDisabled()}>
          <div class="rm-banner rm-banner-muted" role="status" aria-live="polite">
            Resource monitoring is disabled.
          </div>
        </Show>

        <Show when={resourceMonitorStore.error}>
          <div class="rm-banner rm-banner-error" role="status" aria-live="polite">
            Snapshot failed: {resourceMonitorStore.error}
          </div>
        </Show>

        <Show when={resourceMonitorStore.stale && snapshot()}>
          <div class="rm-banner rm-banner-muted" role="status" aria-live="polite">
            Showing last snapshot from {formatTimestamp(snapshot()?.capturedAt)}.
          </div>
        </Show>

        <section class="rm-groups">
          <div class="rm-section-header">
            {/* tabindex="-1" so a closing kill modal whose group has vanished
                from the snapshot has somewhere to return focus. Never body. */}
            <h2
              ref={groupsHeadingRef}
              tabindex="-1"
              data-ac-testid="resourceMonitor.groups.heading"
              data-ac-role="text"
            >
              Agents
            </h2>
            <div class="rm-section-header-meta">
              {/* The single counter: always rendered, never gated on
                  filtersActive(), so a clean state reads the agent total
                  instead of disappearing. Exactly one element carries
                  aria-live, so a filter change is announced once. */}
              <span
                class="rm-filter-count"
                role="status"
                aria-live="polite"
                data-ac-testid="resourceMonitor.filter.count"
                data-ac-role="text"
              >
                {counterText()}
              </span>
              <span
                data-ac-testid="resourceMonitor.summary.timestamp"
                data-ac-role="text"
              >
                Last update {formatTimestamp(snapshot()?.capturedAt)}
              </span>
            </div>
          </div>

          <div
            class="rm-filter-bar"
            data-ac-testid="resourceMonitor.filter"
            data-ac-role="toolbar"
          >
            <div
              class="rm-filter-segment"
              role="group"
              aria-label="Filter by status"
              data-ac-testid="resourceMonitor.filter.status"
              data-ac-role="group"
            >
              <For each={STATUS_FILTERS}>
                {(option) => (
                  <button
                    type="button"
                    class="rm-filter-seg-btn"
                    classList={{ "is-active": statusFilter() === option.value }}
                    onClick={() => setStatusFilter(option.value)}
                    aria-pressed={statusFilter() === option.value}
                    data-ac-testid={`resourceMonitor.filter.status.${option.value}`}
                    data-ac-role="button"
                    data-ac-state={
                      statusFilter() === option.value ? "active" : "inactive"
                    }
                  >
                    {option.label}
                  </button>
                )}
              </For>
            </div>

            <Show when={projectOptions().length > 0}>
              <div
                class="rm-filter-group"
                role="group"
                aria-label="Filter by project"
                data-ac-testid="resourceMonitor.filter.project"
                data-ac-role="group"
              >
                <span class="rm-filter-label">Project</span>
                <For each={projectOptions()}>
                  {(value) => (
                    <button
                      type="button"
                      class="rm-filter-chip"
                      classList={{ "is-active": projectFilter().has(value) }}
                      onClick={() =>
                        toggleFilter(projectFilter, setProjectFilter, value)
                      }
                      aria-pressed={projectFilter().has(value)}
                      data-ac-testid={`resourceMonitor.filter.project.${value}`}
                      data-ac-role="button"
                      data-ac-state={
                        projectFilter().has(value) ? "active" : "inactive"
                      }
                    >
                      {value}
                    </button>
                  )}
                </For>
              </div>
            </Show>

            <Show when={workgroupOptions().length > 0}>
              <div
                class="rm-filter-group"
                role="group"
                aria-label="Filter by room"
                data-ac-testid="resourceMonitor.filter.workgroup"
                data-ac-role="group"
              >
                <span class="rm-filter-label">Room</span>
                <For each={workgroupOptions()}>
                  {(value) => (
                    <button
                      type="button"
                      class="rm-filter-chip"
                      classList={{ "is-active": workgroupFilter().has(value) }}
                      onClick={() =>
                        toggleFilter(workgroupFilter, setWorkgroupFilter, value)
                      }
                      aria-pressed={workgroupFilter().has(value)}
                      data-ac-testid={`resourceMonitor.filter.workgroup.${value}`}
                      data-ac-role="button"
                      data-ac-state={
                        workgroupFilter().has(value) ? "active" : "inactive"
                      }
                    >
                      {value}
                    </button>
                  )}
                </For>
              </div>
            </Show>

            <Show when={roleOptions().length > 0}>
              <div
                class="rm-filter-group"
                role="group"
                aria-label="Filter by role"
                data-ac-testid="resourceMonitor.filter.role"
                data-ac-role="group"
              >
                <span class="rm-filter-label">Role</span>
                <For each={roleOptions()}>
                  {(value) => (
                    <button
                      type="button"
                      class="rm-filter-chip"
                      classList={{ "is-active": roleFilter().has(value) }}
                      onClick={() =>
                        toggleFilter(roleFilter, setRoleFilter, value)
                      }
                      aria-pressed={roleFilter().has(value)}
                      data-ac-testid={`resourceMonitor.filter.role.${value}`}
                      data-ac-role="button"
                      data-ac-state={
                        roleFilter().has(value) ? "active" : "inactive"
                      }
                    >
                      {value}
                    </button>
                  )}
                </For>
              </div>
            </Show>

            <div
              class="rm-filter-group rm-filter-pid"
              data-ac-testid="resourceMonitor.filter.pid"
              data-ac-role="group"
            >
              <label class="rm-filter-label" for="rm-filter-pid-input">
                PID
              </label>
              <input
                id="rm-filter-pid-input"
                class="rm-filter-pid-input"
                type="text"
                inputmode="numeric"
                value={pidText()}
                disabled={monitorDisabled()}
                title={monitorDisabled() ? MONITOR_DISABLED_TITLE : PID_HELP_TEXT}
                aria-describedby="rm-filter-pid-help"
                aria-invalid={pidParse().rejected.length > 0 ? "true" : undefined}
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setPidText(value);
                  pidDebounce.schedule(value);
                }}
                data-ac-testid="resourceMonitor.filter.pid.input"
                data-ac-role="input"
              />
              <span id="rm-filter-pid-help" class="rm-filter-help">
                {PID_HELP_TEXT}
              </span>
              <Show when={pidText().length > 0}>
                <button
                  type="button"
                  class="rm-filter-input-clear"
                  onClick={clearPidFilter}
                  aria-label="Clear the PID filter"
                  data-ac-testid="resourceMonitor.filter.pid.clear"
                  data-ac-role="button"
                >
                  &#x2715;
                </button>
              </Show>
              <For each={pidParse().pids}>
                {(pid) => (
                  <button
                    type="button"
                    class="rm-pid-chip"
                    classList={{ "is-unmatched": !pidPresentInSnapshot(pid) }}
                    onClick={() => {
                      const next = pidParse()
                        .pids.filter((value) => value !== pid)
                        .join(", ");
                      setPidText(next);
                      pidDebounce.flush(next);
                    }}
                    aria-label={`Remove PID ${pid}`}
                    title={
                      pidPresentInSnapshot(pid) ? undefined : PID_UNMATCHED_TITLE
                    }
                    data-ac-testid={`resourceMonitor.filter.pid.chip.${pid}`}
                    data-ac-role="button"
                    data-ac-state={
                      pidPresentInSnapshot(pid) ? "matched" : "unmatched"
                    }
                  >
                    <span class="rm-pid-chip-pid">{pid}</span>
                    {/* 11.1: PID reuse is real and untestable from here. Naming
                        the process is what exposes the confusion to the user. */}
                    <Show when={pidProcessName(pid)}>
                      <span class="rm-pid-chip-name">{pidProcessName(pid)}</span>
                    </Show>
                  </button>
                )}
              </For>
              <Show when={pidNoticeText().length > 0}>
                <span
                  class="rm-filter-pid-error"
                  role="status"
                  aria-live="polite"
                  data-ac-testid="resourceMonitor.filter.pid.error"
                  data-ac-role="status"
                  data-ac-state={pidNoticeState()}
                >
                  {pidNoticeText()}
                </span>
              </Show>
            </div>

            <div
              class="rm-filter-group rm-filter-search"
              data-ac-testid="resourceMonitor.filter.search"
              data-ac-role="group"
            >
              <label class="rm-filter-label" for="rm-filter-search-input">
                Search
              </label>
              <input
                id="rm-filter-search-input"
                class="rm-filter-search-input"
                type="text"
                value={searchText()}
                disabled={monitorDisabled()}
                title={
                  monitorDisabled()
                    ? MONITOR_DISABLED_TITLE
                    : "Search agent, room, project, role and process names."
                }
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setSearchText(value);
                  searchDebounce.schedule(value);
                }}
                data-ac-testid="resourceMonitor.filter.search.input"
                data-ac-role="input"
              />
              <Show when={searchText().length > 0}>
                <button
                  type="button"
                  class="rm-filter-input-clear"
                  onClick={clearSearchFilter}
                  aria-label="Clear the search filter"
                  data-ac-testid="resourceMonitor.filter.search.clear"
                  data-ac-role="button"
                >
                  &#x2715;
                </button>
              </Show>
            </div>

            {/* The trailing container carries the margin-left:auto that used to
                sit on .rm-filter-clear; without it the adjacent controls part. */}
            <div class="rm-filter-trailing">
              <div
                class="rm-sort"
                data-ac-testid="resourceMonitor.sort"
                data-ac-role="group"
              >
                <label class="rm-filter-label" for="rm-sort-field">
                  Sort
                </label>
                <select
                  id="rm-sort-field"
                  class="rm-sort-field"
                  value={sortField()}
                  onChange={(event) =>
                    changeSortField(event.currentTarget.value as RmSortField)
                  }
                  data-ac-testid="resourceMonitor.sort.field"
                  data-ac-role="input"
                >
                  <For each={SORT_OPTIONS}>
                    {(option) => <option value={option.value}>{option.label}</option>}
                  </For>
                </select>
                <button
                  type="button"
                  class="rm-sort-direction"
                  disabled={sortField() === "default"}
                  onClick={toggleSortDirection}
                  aria-label={
                    sortDirection() === "asc"
                      ? "Sort ascending"
                      : "Sort descending"
                  }
                  data-ac-testid="resourceMonitor.sort.direction"
                  data-ac-role="button"
                  data-ac-state={
                    sortField() === "default" ? "disabled" : sortDirection()
                  }
                >
                  {sortDirection() === "asc" ? "^" : "v"}
                </button>
              </div>

              <Show when={filtersActive()}>
                <button
                  type="button"
                  class="rm-filter-clear"
                  onClick={clearFilters}
                  data-ac-testid="resourceMonitor.filter.clear"
                  data-ac-role="button"
                >
                  Clear filters
                </button>
              </Show>
            </div>
          </div>

          <Show when={coverageNoticeVisible()}>
            {/* Evaluated over the groups passing the NON-PID filters: the useful
                case is a PID missing because its tree is partial, and that group
                is exactly the one the PID filter has removed from view. */}
            <div
              class="rm-filter-coverage"
              role="status"
              aria-live="polite"
              data-ac-testid="resourceMonitor.filter.coverage"
              data-ac-role="status"
            >
              {COVERAGE_NOTICE}
            </div>
          </Show>

          <Show
            when={sortedGroups().length > 0}
            fallback={
              <div
                class="rm-empty"
                data-ac-testid="resourceMonitor.empty"
                data-ac-role="status"
                data-ac-state={
                  resourceMonitorStore.loading && groups().length === 0
                    ? "loading"
                    : groups().length === 0
                      ? "empty"
                      : "filtered-empty"
                }
              >
                {emptyStateText()}
              </div>
            }
          >
            <div class="rm-group-list">
              <For each={sortedGroups()}>
                {(group) => (
                  <div
                    class={`rm-group-row state-${groupSeverity(group)}`}
                    classList={{ "is-expanded": isExpanded(group.sessionId) }}
                    data-ac-testid={`resourceMonitor.group.${group.sessionId}`}
                    data-ac-role="group"
                    data-ac-state={groupSeverity(group)}
                  >
                    <button
                      class="rm-group-main"
                      onClick={() => toggleGroup(group.sessionId)}
                      aria-expanded={isExpanded(group.sessionId)}
                      data-ac-testid={`resourceMonitor.group.${group.sessionId}.toggle`}
                      data-ac-role="button"
                    >
                      <span class="rm-expander">
                        {isExpanded(group.sessionId) ? "v" : ">"}
                      </span>
                      <span class="rm-group-identity">
                        {/* The pill rides beside the name on its own flex line:
                            .rm-group-main is a fixed grid and .rm-group-identity
                            a flex column, so appending it to either would put it
                            in an implicit row or on a line of its own. A span,
                            never a div: this sits inside a <button>. */}
                        <span class="rm-group-identity-line">
                          <span
                            class="rm-group-name"
                            data-ac-testid={`resourceMonitor.group.${group.sessionId}.name`}
                            data-ac-role="cell"
                          >
                            {group.name}
                          </span>
                          <Show when={group.descendantsObserved === false}>
                            <span
                              class="rm-partial-pill"
                              title={PARTIAL_PILL_TITLE}
                              data-ac-testid={`resourceMonitor.group.${group.sessionId}.partial`}
                              data-ac-role="status"
                            >
                              Partial
                            </span>
                          </Show>
                        </span>
                        <span
                          class="rm-group-origin"
                          title={groupOrigin(group)}
                          data-ac-testid={`resourceMonitor.group.${group.sessionId}.origin`}
                          data-ac-role="cell"
                        >
                          {groupOrigin(group)}
                        </span>
                        <Show when={isVerifying(group)}>
                          <span
                            class="rm-group-verifying"
                            data-ac-testid={`resourceMonitor.group.${group.sessionId}.verifying`}
                            data-ac-role="status"
                          >
                            Verifying...
                          </span>
                        </Show>
                      </span>
                      <span
                        class="rm-group-state"
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.state`}
                        data-ac-role="cell"
                        data-ac-state={group.state}
                      >
                        {group.state}
                      </span>
                      <span
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.processCount`}
                        data-ac-role="cell"
                      >
                        {group.processCount} proc
                      </span>
                      <span
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.privateBytes`}
                        data-ac-role="cell"
                      >
                        {formatBytes(group.privateBytes)}
                      </span>
                      {/* #2245: this metric used to be visually hidden at every
                          width, so "no metric disappears" was false before the
                          change started. It now takes the eighth grid track. */}
                      <span
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.workingSetBytes`}
                        data-ac-role="cell"
                      >
                        {formatBytes(group.workingSetBytes)}
                      </span>
                      <span
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.cpu`}
                        data-ac-role="cell"
                      >
                        {formatCpu(group.cpuPercent)}
                      </span>
                      <span
                        class={`rm-network-pill network-${group.networkState}`}
                        title={group.networkSummary || group.networkState}
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.network`}
                        data-ac-role="cell"
                        data-ac-state={group.networkState}
                      >
                        {group.networkSummary || group.networkState}
                      </span>
                    </button>
                    <button
                      class="rm-kill-btn"
                      classList={{
                        "rm-kill-btn-force": group.state === "quarantined",
                      }}
                      disabled={!canKillGroup(group)}
                      onClick={() => openKillModal(group)}
                      data-ac-testid={`resourceMonitor.group.${group.sessionId}.kill`}
                      data-ac-role="button"
                      data-ac-state={canKillGroup(group) ? "ready" : "disabled"}
                    >
                      {killActionLabel(group)}
                    </button>

                    <Show when={isExpanded(group.sessionId)}>
                      <div
                        class="rm-process-list"
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.processList`}
                        data-ac-role="row"
                        data-ac-state={`${group.processes.length}`}
                      >
                        <div class="rm-process-header">
                          <span>Process</span>
                          <span>PID</span>
                          <span>Private</span>
                          <span>Working Set</span>
                          <span>CPU</span>
                          <span>Kill Scope</span>
                        </div>
                        <For
                          each={group.processes}
                          fallback={
                            <div
                              class="rm-process-empty"
                              data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.empty`}
                              data-ac-role="status"
                            >
                              No processes observed
                            </div>
                          }
                        >
                          {(process) => (
                            <div
                              class="rm-process-row"
                              classList={{
                                "is-pid-match": appliedPids().has(process.pid),
                              }}
                              title={processRowTitle(process)}
                              data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}`}
                              data-ac-role="row"
                              data-ac-state={
                                appliedPids().has(process.pid)
                                  ? "pid-match"
                                  : undefined
                              }
                            >
                              {/* Depth is an inline padding rather than a class
                                  ladder or a calc(): the tree is flat when depth
                                  is absent, which is the intended degradation. */}
                              <span
                                classList={{
                                  "rm-process-depth": (process.depth ?? 0) > 0,
                                }}
                                style={{ "padding-left": depthPadding(process.depth) }}
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.name`}
                                data-ac-role="cell"
                              >
                                {processName(process)}
                              </span>
                              <span
                                classList={{
                                  "rm-process-pid-match": appliedPids().has(
                                    process.pid
                                  ),
                                }}
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.pid`}
                                data-ac-role="cell"
                              >
                                {process.pid}
                              </span>
                              <span
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.privateBytes`}
                                data-ac-role="cell"
                              >
                                {formatBytes(process.privateBytes)}
                              </span>
                              <span
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.workingSetBytes`}
                                data-ac-role="cell"
                              >
                                {formatBytes(process.workingSetBytes)}
                              </span>
                              <span
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.cpu`}
                                data-ac-role="cell"
                              >
                                {formatCpu(process.cpuPercent)}
                              </span>
                              <span
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.killAllowed`}
                                data-ac-role="cell"
                                data-ac-state={process.killAllowed ? "allowed" : "blocked"}
                              >
                                {process.killAllowed ? "Allowed" : "Blocked"}
                              </span>
                            </div>
                          )}
                        </For>
                        <Show when={group.lastError}>
                          <div
                            class="rm-process-error"
                            data-ac-testid={`resourceMonitor.group.${group.sessionId}.lastError`}
                            data-ac-role="status"
                          >
                            {group.lastError}
                          </div>
                        </Show>
                        {/* #647 D: ADD the security guidance below the per-PID
                            detail (never replacing it) when the last kill on this
                            still-quarantined group was blocked by a security
                            product. */}
                        <Show
                          when={
                            group.state === "quarantined" &&
                            killResult()?.sessionId === group.sessionId &&
                            killResult()?.blockedBySecurity
                          }
                        >
                          <div
                            class="rm-process-error rm-security-hint"
                            data-ac-testid={`resourceMonitor.group.${group.sessionId}.securityHint`}
                            data-ac-role="status"
                          >
                            {SECURITY_BLOCK_HINT}
                          </div>
                        </Show>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </section>

        <Show when={(snapshot()?.warnings ?? []).length > 0}>
          <section class="rm-warnings">
            <div class="rm-section-header">
              <h2>Warnings</h2>
            </div>
            <For each={snapshot()?.warnings ?? []}>
              {(warning, index) => (
                <div
                  class="rm-warning-line"
                  data-ac-testid={`resourceMonitor.warning.${index()}`}
                  data-ac-role="status"
                >
                  {warning}
                </div>
              )}
            </For>
          </section>
        </Show>
      </main>

      <Show when={killTarget()} keyed>
        {(target) => {
          // Initial focus goes to Cancel, never to the destructive button.
          onMount(() => cancelRef?.focus());
          return (
          <div
            class="rm-modal-backdrop"
            // The guard is load-bearing, not decoration: mousedown bubbles up
            // from the dialog, so an unguarded preventDefault here would also
            // cancel every press INSIDE the modal and kill text selection of
            // the group name and the error text. Without the handler at all a
            // backdrop click blurs into body, after which Escape and the trap
            // are both dead, because the handler lives on the dialog element.
            onMouseDown={(event) => {
              if (event.target === event.currentTarget) event.preventDefault();
            }}
            data-ac-testid="resourceMonitor.killConfirm"
          >
            <div
              ref={modalRef}
              class="rm-modal"
              role="dialog"
              aria-modal="true"
              aria-labelledby={KILL_MODAL_TITLE_ID}
              tabindex="-1"
              onKeyDown={(event) => handleModalKeyDown(event, target.sessionId)}
              data-ac-testid="resourceMonitor.killConfirm.dialog"
              data-ac-role="dialog"
            >
              <h2 id={KILL_MODAL_TITLE_ID}>
                {target.state === "quarantined" ? "Force-kill agent" : "Kill agent"}
              </h2>
              <p
                class="rm-modal-target"
                title={groupOrigin(target)}
                data-ac-testid="resourceMonitor.killConfirm.origin"
                data-ac-role="text"
              >
                {groupOrigin(target)}
              </p>
              <p
                class="rm-modal-detail"
                data-ac-testid="resourceMonitor.killConfirm.name"
                data-ac-role="text"
              >
                {target.name}
              </p>
              <p class="rm-modal-detail">
                Session {target.sessionId} and its entire process tree will be
                force-terminated via its Job Object.
              </p>
              {/* #647 (Step 7): a non-finalized result keeps the modal open. A
                  `terminating` result means a concurrent kill is still settling
                  (show "Verifying...", offer Retry); otherwise it is blocked
                  (`quarantined`) — show the per-PID detail, with the AV-exclusion
                  hint PREPENDED above it (never replacing it) when blocked by a
                  security product. */}
              <Show when={killResult()} keyed>
                {(res) =>
                  res.state === "terminating" ? (
                    <div
                      class="rm-banner rm-banner-muted"
                      data-ac-testid="resourceMonitor.killConfirm.verifying"
                      data-ac-role="status"
                    >
                      Verifying... a concurrent kill is still settling. Click Retry
                      to confirm.
                    </div>
                  ) : (
                    <div
                      class="rm-banner rm-banner-error"
                      data-ac-testid="resourceMonitor.killConfirm.quarantined"
                      data-ac-role="status"
                    >
                      <Show when={res.blockedBySecurity}>
                        <div
                          class="rm-security-hint"
                          data-ac-testid="resourceMonitor.killConfirm.securityHint"
                          data-ac-role="status"
                        >
                          {SECURITY_BLOCK_HINT}
                        </div>
                      </Show>
                      <div data-ac-testid="resourceMonitor.killConfirm.message">
                        {res.message}
                      </div>
                    </div>
                  )
                }
              </Show>
              <Show when={killError()}>
                <div class="rm-banner rm-banner-error">{killError()}</div>
              </Show>
              <div class="rm-modal-actions">
                <button
                  ref={cancelRef}
                  class="rm-action-btn"
                  disabled={killInFlight()}
                  onClick={() => void closeKillModal(target.sessionId)}
                  data-ac-testid="resourceMonitor.killConfirm.cancel"
                  data-ac-role="button"
                >
                  {killResult() ? "Close" : "Cancel"}
                </button>
                <button
                  class="rm-action-btn rm-action-danger"
                  disabled={killInFlight()}
                  onClick={confirmKill}
                  data-ac-testid="resourceMonitor.killConfirm.confirm"
                  data-ac-role="button"
                >
                  {killInFlight()
                    ? "Verifying..."
                    : killResult()
                      ? "Retry"
                      : target.state === "quarantined"
                        ? "Force-kill"
                        : "Kill Agent"}
                </button>
              </div>
            </div>
          </div>
          );
        }}
      </Show>
    </div>
  );
};

export default ResourceMonitorApp;
