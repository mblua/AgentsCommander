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
import { FILTER_DEBOUNCE_MS, MAX_PIDS, parsePidFilter } from "../shared/pid-filter";
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

type RmSortField = "default" | "cpu" | "private" | "processes" | "name";
type RmSortDirection = "asc" | "desc";

interface DebouncedApply {
  schedule(value: string): void;
  flush(value: string): void;
  cancel(): void;
}

const debouncedApply = (setApplied: (value: string) => void): DebouncedApply => {
  let handle: ReturnType<typeof setTimeout> | null = null;

  const clear = () => {
    if (handle !== null) {
      clearTimeout(handle);
      handle = null;
    }
  };

  return {
    schedule(value: string) {
      clear();
      handle = setTimeout(() => {
        handle = null;
        setApplied(value);
      }, FILTER_DEBOUNCE_MS);
    },
    flush(value: string) {
      clear();
      setApplied(value);
    },
    cancel: clear,
  };
};

const processTitle = (process: ResourceProcessSnapshot): string =>
  typeof process.parentPid === "number"
    ? `PID ${process.pid} - parent ${process.parentPid}`
    : `PID ${process.pid}`;

const sortMetric = (
  group: ResourceAgentGroupSnapshot,
  field: RmSortField,
): number | null => {
  if (field === "cpu") {
    return typeof group.cpuPercent === "number" ? group.cpuPercent : null;
  }
  if (field === "private") {
    return typeof group.privateBytes === "number" ? group.privateBytes : null;
  }
  if (field === "processes") return group.processCount;
  return null;
};

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

  const openKillModal = (group: ResourceAgentGroupSnapshot) => {
    setKillError("");
    setKillResult(null);
    setKillTarget(group);
  };

  let modalEl: HTMLDivElement | undefined;

  const restoreFocus = (sessionId: string) => {
    queueMicrotask(() => {
      const killButton = document.querySelector<HTMLElement>(
        `[data-ac-testid="resourceMonitor.group.${sessionId}.kill"]`
      );
      if (killButton) {
        killButton.focus();
        return;
      }
      document
        .querySelector<HTMLElement>(
          '[data-ac-testid="resourceMonitor.groups.heading"]'
        )
        ?.focus();
    });
  };

  const closeKillModal = (sessionId: string) => {
    setKillTarget(null);
    setKillError("");
    restoreFocus(sessionId);
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
  const processTotal = createMemo(() =>
    groups().reduce((total, group) => total + group.processCount, 0)
  );

  const [statusFilter, setStatusFilter] = createSignal<RmStatusFilter>("all");
  const [projectFilter, setProjectFilter] = createSignal<Set<string>>(new Set());
  const [workgroupFilter, setWorkgroupFilter] = createSignal<Set<string>>(
    new Set()
  );
  const [roleFilter, setRoleFilter] = createSignal<Set<string>>(new Set());

  const [pidFilterText, setPidFilterText] = createSignal("");
  const [appliedPidText, setAppliedPidText] = createSignal("");
  const [searchText, setSearchText] = createSignal("");
  const [appliedSearch, setAppliedSearch] = createSignal("");
  const [sortField, setSortField] = createSignal<RmSortField>("default");
  const [sortDirection, setSortDirection] = createSignal<RmSortDirection>("desc");

  const pidDebounce = debouncedApply(setAppliedPidText);
  const searchDebounce = debouncedApply(setAppliedSearch);
  onCleanup(() => {
    pidDebounce.cancel();
    searchDebounce.cancel();
  });

  const parsedPidFilter = createMemo(() => parsePidFilter(appliedPidText()));
  const appliedPidSet = createMemo(() => new Set(parsedPidFilter().pids));

  const projectOptions = createMemo(() =>
    distinct(groups().map((g) => g.project))
  );
  const workgroupOptions = createMemo(() =>
    distinct(groups().map((g) => g.workgroup))
  );
  const roleOptions = createMemo(() => distinct(groups().map((g) => g.agent)));

  const matchesSearch = (
    group: ResourceAgentGroupSnapshot,
    needle: string,
  ): boolean => {
    const fields = [group.name, group.agent, group.workgroup, group.project];
    if (fields.some((field) => (field ?? "").toLowerCase().includes(needle))) {
      return true;
    }
    return group.processes.some((process) =>
      (process.name ?? process.exeName ?? "").toLowerCase().includes(needle)
    );
  };

  const matchesNonPidFilters = (group: ResourceAgentGroupSnapshot): boolean => {
    const status = statusFilter();
    if (status === "active" && !isActiveGroup(group)) return false;
    if (status === "inactive" && isActiveGroup(group)) return false;
    const projects = projectFilter();
    if (projects.size > 0 && !(group.project && projects.has(group.project))) {
      return false;
    }
    const wgs = workgroupFilter();
    if (wgs.size > 0 && !(group.workgroup && wgs.has(group.workgroup))) {
      return false;
    }
    const roles = roleFilter();
    if (roles.size > 0 && !(group.agent && roles.has(group.agent))) return false;
    const query = appliedSearch();
    if (query.length > 0 && !matchesSearch(group, query.toLowerCase())) {
      return false;
    }
    return true;
  };

  const matchesPidSet = (
    group: ResourceAgentGroupSnapshot,
    pids: Set<number>,
  ): boolean => {
    if (pids.size === 0) return true;
    if (typeof group.rootPid === "number" && pids.has(group.rootPid)) return true;
    return group.processes.some((process) => pids.has(process.pid));
  };

  const nonPidFilteredGroups = createMemo(() =>
    groups().filter((group) => matchesNonPidFilters(group))
  );

  const filteredGroups = createMemo(() =>
    nonPidFilteredGroups().filter((group) =>
      matchesPidSet(group, appliedPidSet())
    )
  );

  let pinnedIndex = new Map<string, number>();
  let lastSortSignature = "";
  let lastFilterSignature = "";

  const applySort = (
    list: ResourceAgentGroupSnapshot[]
  ): ResourceAgentGroupSnapshot[] => {
    const field = sortField();
    if (field === "default") return list;
    const direction = sortDirection();
    return [...list].sort((a, b) => {
      if (field === "name") {
        const compared = a.name.localeCompare(b.name);
        if (compared !== 0) return direction === "desc" ? -compared : compared;
        return a.sessionId.localeCompare(b.sessionId);
      }
      const left = sortMetric(a, field);
      const right = sortMetric(b, field);
      if (typeof left === "number" && typeof right === "number") {
        if (left !== right) {
          const compared = left - right;
          return direction === "desc" ? -compared : compared;
        }
      } else if (typeof left === "number") {
        return -1;
      } else if (typeof right === "number") {
        return 1;
      }
      return a.sessionId.localeCompare(b.sessionId);
    });
  };

  const sortedGroups = createMemo(() => {
    const filtered = filteredGroups();
    const field = sortField();
    const direction = sortDirection();
    const expanded = expandedGroupIds();
    const filterSignature = JSON.stringify([
      statusFilter(),
      [...projectFilter()].sort(),
      [...workgroupFilter()].sort(),
      [...roleFilter()].sort(),
      appliedPidText(),
      appliedSearch(),
    ]);
    const sortSignature = `${field}|${direction}`;

    let ordered = applySort(filtered);

    if (
      sortSignature !== lastSortSignature ||
      filterSignature !== lastFilterSignature
    ) {
      pinnedIndex = new Map();
      lastSortSignature = sortSignature;
      lastFilterSignature = filterSignature;
    }

    const present = new Set(ordered.map((group) => group.sessionId));
    for (const sessionId of Array.from(pinnedIndex.keys())) {
      if (!present.has(sessionId) || !expanded.has(sessionId)) {
        pinnedIndex.delete(sessionId);
      }
    }

    if (field !== "default" && pinnedIndex.size > 0) {
      const pinned = Array.from(pinnedIndex.entries()).sort(
        (a, b) => a[1] - b[1]
      );
      const pinnedIds = new Set(pinned.map(([sessionId]) => sessionId));
      const byId = new Map(ordered.map((group) => [group.sessionId, group]));
      const rest = ordered.filter((group) => !pinnedIds.has(group.sessionId));
      for (const [sessionId, index] of pinned) {
        const group = byId.get(sessionId);
        if (!group) continue;
        rest.splice(Math.min(index, rest.length), 0, group);
      }
      ordered = rest;
    }

    for (const sessionId of expanded) {
      if (pinnedIndex.has(sessionId)) continue;
      const index = ordered.findIndex((group) => group.sessionId === sessionId);
      if (index >= 0) pinnedIndex.set(sessionId, index);
    }

    return ordered;
  });

  const pidStats = createMemo(() => {
    const pids = appliedPidSet();
    let matchedProcesses = 0;
    let rootOnlyGroups = 0;
    if (pids.size === 0) return { matchedProcesses, rootOnlyGroups };
    for (const group of sortedGroups()) {
      const processMatches = group.processes.filter((process) =>
        pids.has(process.pid)
      ).length;
      matchedProcesses += processMatches;
      const rootMatches =
        typeof group.rootPid === "number" && pids.has(group.rootPid);
      if (rootMatches && processMatches === 0) rootOnlyGroups += 1;
    }
    return { matchedProcesses, rootOnlyGroups };
  });

  const counterText = createMemo(() => {
    let text = `Showing ${sortedGroups().length} of ${groups().length} agents`;
    if (appliedPidSet().size > 0) {
      const { matchedProcesses, rootOnlyGroups } = pidStats();
      text += ` - ${matchedProcesses} matching ${
        matchedProcesses === 1 ? "process" : "processes"
      }`;
      if (rootOnlyGroups > 0) {
        text += `, ${rootOnlyGroups} matched by root PID only`;
      }
    }
    return text;
  });

  const filtersActive = createMemo(
    () =>
      statusFilter() !== "all" ||
      projectFilter().size > 0 ||
      workgroupFilter().size > 0 ||
      roleFilter().size > 0 ||
      parsedPidFilter().pids.length > 0 ||
      appliedSearch().length > 0
  );

  const pidOnlyFilter = createMemo(
    () =>
      parsedPidFilter().pids.length > 0 &&
      statusFilter() === "all" &&
      projectFilter().size === 0 &&
      workgroupFilter().size === 0 &&
      roleFilter().size === 0 &&
      appliedSearch().length === 0
  );

  const pidNotice = createMemo(() => {
    const parsed = parsedPidFilter();
    const parts: string[] = [];
    if (parsed.rejected.length > 0) {
      parts.push(`Unrecognized PID: ${parsed.rejected.join(", ")}.`);
    }
    if (parsed.truncated) {
      parts.push(`Only the first ${MAX_PIDS} PIDs are applied.`);
    }
    if (parts.length === 0) return null;
    return { text: parts.join(" "), error: parsed.pids.length === 0 };
  });

  const coverageNotice = createMemo(
    () =>
      parsedPidFilter().pids.length > 0 &&
      nonPidFilteredGroups().some((group) => !group.descendantsObserved)
  );

  const chipState = (pid: number): "matched" | "unmatched" => {
    for (const group of groups()) {
      if (typeof group.rootPid === "number" && group.rootPid === pid) {
        return "matched";
      }
      if (group.processes.some((process) => process.pid === pid)) {
        return "matched";
      }
    }
    return "unmatched";
  };

  const removePid = (pid: number) => {
    const remaining = parsedPidFilter()
      .pids.filter((value) => value !== pid)
      .join(", ");
    setPidFilterText(remaining);
    pidDebounce.flush(remaining);
  };

  const clearFilters = () => {
    setStatusFilter("all");
    setProjectFilter(new Set<string>());
    setWorkgroupFilter(new Set<string>());
    setRoleFilter(new Set<string>());
    setPidFilterText("");
    setSearchText("");
    pidDebounce.flush("");
    searchDebounce.flush("");
  };

  const statusClass = createMemo(() => {
    const s = snapshot();
    if (!s || s.overallState === "unknown") return "unknown";
    if (s.overallState === "ok" && s.networkState === "unknown") return "unknown";
    return s.overallState;
  });
  const statusText = createMemo(() =>
    overallLabel(snapshot()?.overallState ?? "unknown")
  );
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

  let lastAppliedPidSignature = "";
  createEffect(() => {
    const signature = parsedPidFilter().pids.join(",");
    if (signature === lastAppliedPidSignature) return;
    lastAppliedPidSignature = signature;
    const pids = appliedPidSet();
    if (pids.size === 0) return;
    const matching = groups()
      .filter((group) => matchesPidSet(group, pids))
      .map((group) => group.sessionId);
    if (matching.length === 0) return;
    setExpandedGroupIds((current) => {
      const next = new Set(current);
      for (const sessionId of matching) next.add(sessionId);
      return next;
    });
  });

  const handleSortFieldChange = (field: RmSortField) => {
    setSortField(field);
    setSortDirection(field === "name" ? "asc" : "desc");
  };

  const toggleSortDirection = () => {
    if (sortField() === "default") return;
    setSortDirection((current) => (current === "desc" ? "asc" : "desc"));
  };

  createEffect(() => {
    if (killInFlight() && killTarget() && modalEl) modalEl.focus();
  });

  const handleModalKeyDown = (event: KeyboardEvent) => {
    const target = killTarget();
    if (event.key === "Escape") {
      event.preventDefault();
      if (killInFlight() || !target) return;
      closeKillModal(target.sessionId);
      return;
    }
    if (event.key !== "Tab") return;
    const focusables = modalEl
      ? Array.from(modalEl.querySelectorAll<HTMLElement>("button:not([disabled])"))
      : [];
    event.preventDefault();
    if (focusables.length === 0) {
      modalEl?.focus();
      return;
    }
    const current = document.activeElement as HTMLElement | null;
    const index = current ? focusables.indexOf(current) : -1;
    const next = event.shiftKey
      ? focusables[index <= 0 ? focusables.length - 1 : index - 1]
      : focusables[(index + 1) % focusables.length];
    next?.focus();
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
        await resourceMonitorStore.refresh();
        closeKillModal(target.sessionId);
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
            <strong>{processTotal()}</strong>
          </div>
          <div
            class="rm-status-tile"
            data-ac-testid="resourceMonitor.summary.appPrivateBytes"
            data-ac-role="metric"
          >
            <span class="rm-tile-label">App Private</span>
            <strong>{formatBytes(snapshot()?.appPrivateBytes)}</strong>
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

        <Show when={snapshot()?.monitorEnabled === false}>
          <div
            class="rm-banner rm-banner-muted"
            role="status"
            aria-live="polite"
          >
            Resource monitoring is disabled.
          </div>
        </Show>

        <Show when={resourceMonitorStore.error}>
          <div
            class="rm-banner rm-banner-error"
            role="status"
            aria-live="polite"
          >
            Snapshot failed: {resourceMonitorStore.error}
          </div>
        </Show>

        <Show when={resourceMonitorStore.stale && snapshot()}>
          <div
            class="rm-banner rm-banner-muted"
            role="status"
            aria-live="polite"
          >
            Showing last snapshot from {formatTimestamp(snapshot()?.capturedAt)}.
          </div>
        </Show>

        <section class="rm-groups">
          <div class="rm-section-header">
            <h2 tabindex="-1" data-ac-testid="resourceMonitor.groups.heading">
              Agents
            </h2>
            <div class="rm-section-header-meta">
              <span
                class="rm-filter-count"
                data-ac-testid="resourceMonitor.filter.count"
                data-ac-role="text"
                role="status"
                aria-live="polite"
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
              role="group"
              aria-label="Filter by PID"
              data-ac-testid="resourceMonitor.filter.pid"
              data-ac-role="group"
            >
              <label class="rm-filter-label" for="rm-filter-pid-input">
                PID
              </label>
              <input
                id="rm-filter-pid-input"
                type="text"
                class="rm-filter-pid-input"
                value={pidFilterText()}
                placeholder="4242, 5120"
                disabled={snapshot()?.monitorEnabled === false}
                title={
                  snapshot()?.monitorEnabled === false
                    ? "Resource monitoring is disabled"
                    : "Filter agents by process ID"
                }
                aria-describedby="rm-filter-pid-help"
                aria-invalid={parsedPidFilter().rejected.length > 0}
                onInput={(event) => {
                  const value = event.currentTarget.value;
                  setPidFilterText(value);
                  pidDebounce.schedule(value);
                }}
                data-ac-testid="resourceMonitor.filter.pid.input"
                data-ac-role="input"
              />
              <span class="rm-filter-help" id="rm-filter-pid-help">
                Comma, semicolon or space separated
              </span>
              <Show when={pidFilterText().length > 0}>
                <button
                  type="button"
                  class="rm-filter-input-clear"
                  onClick={() => {
                    setPidFilterText("");
                    pidDebounce.flush("");
                  }}
                  aria-label="Clear PID filter"
                  data-ac-testid="resourceMonitor.filter.pid.clear"
                  data-ac-role="button"
                >
                  &#x2715;
                </button>
              </Show>
              <For each={parsedPidFilter().pids}>
                {(pid) => (
                  <button
                    type="button"
                    class="rm-pid-chip"
                    classList={{
                      "is-unmatched": chipState(pid) === "unmatched",
                    }}
                    onClick={() => removePid(pid)}
                    aria-label={`Remove PID ${pid}`}
                    title={
                      chipState(pid) === "unmatched"
                        ? "Not present in the observed processes of this snapshot"
                        : undefined
                    }
                    data-ac-testid={`resourceMonitor.filter.pid.chip.${pid}`}
                    data-ac-role="button"
                    data-ac-state={chipState(pid)}
                  >
                    <span>{pid}</span>
                  </button>
                )}
              </For>
              <Show when={pidNotice()}>
                {(notice) => (
                  <span
                    class="rm-filter-notice"
                    classList={{ "is-error": notice().error }}
                    role="status"
                    aria-live="polite"
                    data-ac-testid="resourceMonitor.filter.pid.error"
                    data-ac-role="status"
                  >
                    {notice().text}
                  </span>
                )}
              </Show>
            </div>

            <div
              class="rm-filter-group rm-filter-search"
              role="group"
              aria-label="Search agents"
              data-ac-testid="resourceMonitor.filter.search"
              data-ac-role="group"
            >
              <label class="rm-filter-label" for="rm-filter-search-input">
                Search
              </label>
              <input
                id="rm-filter-search-input"
                type="text"
                class="rm-filter-search-input"
                value={searchText()}
                placeholder="name, room, project"
                disabled={snapshot()?.monitorEnabled === false}
                title={
                  snapshot()?.monitorEnabled === false
                    ? "Resource monitoring is disabled"
                    : "Search agents and their processes"
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
                  onClick={() => {
                    setSearchText("");
                    searchDebounce.flush("");
                  }}
                  aria-label="Clear search"
                  data-ac-testid="resourceMonitor.filter.search.clear"
                  data-ac-role="button"
                >
                  &#x2715;
                </button>
              </Show>
            </div>

            <div class="rm-filter-actions">
              <div
                class="rm-sort"
                role="group"
                aria-label="Sort agents"
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
                    handleSortFieldChange(
                      event.currentTarget.value as RmSortField
                    )
                  }
                  data-ac-testid="resourceMonitor.sort.field"
                  data-ac-role="input"
                >
                  <option value="default">Default</option>
                  <option value="cpu">CPU</option>
                  <option value="private">Private</option>
                  <option value="processes">Processes</option>
                  <option value="name">Name</option>
                </select>
                <button
                  type="button"
                  class="rm-sort-direction"
                  disabled={sortField() === "default"}
                  onClick={toggleSortDirection}
                  aria-label="Sort direction"
                  data-ac-testid="resourceMonitor.sort.direction"
                  data-ac-role="button"
                  data-ac-state={
                    sortField() === "default" ? "disabled" : sortDirection()
                  }
                >
                  {sortDirection() === "desc" ? "\u2193" : "\u2191"}
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

          <Show when={coverageNotice()}>
            <div
              class="rm-banner rm-banner-muted"
              role="status"
              aria-live="polite"
              data-ac-testid="resourceMonitor.filter.coverage"
              data-ac-role="status"
            >
              Some process trees are only partially observed; a PID may be
              missing from this snapshot.
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
                {resourceMonitorStore.loading && groups().length === 0
                  ? "Loading snapshot..."
                  : groups().length === 0
                    ? "No active agents"
                    : pidOnlyFilter()
                      ? `No process in this snapshot matches PID ${parsedPidFilter().pids.join(
                          ", "
                        )}${
                          resourceMonitorStore.stale
                            ? ` Snapshot captured at ${formatTimestamp(
                                snapshot()?.capturedAt
                              )}`
                            : ""
                        }`
                      : "No agents match the filters"}
              </div>
            }
          >
            <div class="rm-group-list">
              <For each={sortedGroups()}>
                {(group) => (
                  <div
                    class={`rm-group-row state-${groupSeverity(group)}`}
                    classList={{
                      "is-expanded": expandedGroupIds().has(group.sessionId),
                    }}
                    data-ac-testid={`resourceMonitor.group.${group.sessionId}`}
                    data-ac-role="group"
                    data-ac-state={groupSeverity(group)}
                  >
                    <button
                      class="rm-group-main"
                      onClick={() => toggleGroup(group.sessionId)}
                      aria-expanded={expandedGroupIds().has(group.sessionId)}
                      data-ac-testid={`resourceMonitor.group.${group.sessionId}.toggle`}
                      data-ac-role="button"
                    >
                      <span class="rm-expander">
                        {expandedGroupIds().has(group.sessionId) ? "v" : ">"}
                      </span>
                      <span class="rm-group-identity">
                        <span class="rm-group-identity-line">
                          <span
                            class="rm-group-name"
                            data-ac-testid={`resourceMonitor.group.${group.sessionId}.name`}
                            data-ac-role="cell"
                          >
                            {group.name}
                          </span>
                          <Show when={!group.descendantsObserved}>
                            <span
                              class="rm-partial-pill"
                              title="Not all descendants were observed in this snapshot"
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

                    <Show when={expandedGroupIds().has(group.sessionId)}>
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
                                "is-pid-match": appliedPidSet().has(process.pid),
                              }}
                              title={processTitle(process)}
                              data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}`}
                              data-ac-role="row"
                              data-ac-state={
                                appliedPidSet().has(process.pid)
                                  ? "pid-match"
                                  : undefined
                              }
                            >
                              <span
                                classList={{
                                  "is-tree": (process.depth ?? 0) > 0,
                                }}
                                style={{
                                  "padding-left": `${12 * Math.min(
                                    process.depth ?? 0,
                                    6
                                  )}px`,
                                }}
                                data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}.name`}
                                data-ac-role="cell"
                              >
                                {processName(process)}
                              </span>
                              <span
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
        {(target) => (
          <div
            class="rm-modal-backdrop"
            onMouseDown={(event) => {
              if (event.target === event.currentTarget) event.preventDefault();
            }}
            data-ac-testid="resourceMonitor.killConfirm"
          >
            <div
              class="rm-modal"
              role="dialog"
              aria-modal="true"
              tabindex="-1"
              aria-labelledby="rm-kill-modal-title"
              ref={(element) => {
                modalEl = element;
              }}
              onKeyDown={handleModalKeyDown}
              data-ac-testid="resourceMonitor.killConfirm.dialog"
              data-ac-role="dialog"
            >
              <h2 id="rm-kill-modal-title">
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
                  ref={(element) => {
                    queueMicrotask(() => element.focus());
                  }}
                  class="rm-action-btn"
                  disabled={killInFlight()}
                  onClick={() => closeKillModal(target.sessionId)}
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
        )}
      </Show>
    </div>
  );
};

export default ResourceMonitorApp;
