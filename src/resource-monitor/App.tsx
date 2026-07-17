import {
  Component,
  For,
  Show,
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
  [...new Set(values.filter((v): v is string => !!v))].sort();

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
  const [expandedGroupId, setExpandedGroupId] = createSignal<string | null>(null);
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

  const closeKillModal = () => {
    setKillTarget(null);
    setKillError("");
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

  const projectOptions = createMemo(() =>
    distinct(groups().map((g) => g.project))
  );
  const workgroupOptions = createMemo(() =>
    distinct(groups().map((g) => g.workgroup))
  );
  const roleOptions = createMemo(() => distinct(groups().map((g) => g.agent)));

  const filteredGroups = createMemo(() => {
    const status = statusFilter();
    const projects = projectFilter();
    const wgs = workgroupFilter();
    const roles = roleFilter();
    return groups().filter((g) => {
      if (status === "active" && !isActiveGroup(g)) return false;
      if (status === "inactive" && isActiveGroup(g)) return false;
      if (projects.size > 0 && !(g.project && projects.has(g.project)))
        return false;
      if (wgs.size > 0 && !(g.workgroup && wgs.has(g.workgroup))) return false;
      if (roles.size > 0 && !(g.agent && roles.has(g.agent))) return false;
      return true;
    });
  });
  const filtersActive = createMemo(
    () =>
      statusFilter() !== "all" ||
      projectFilter().size > 0 ||
      workgroupFilter().size > 0 ||
      roleFilter().size > 0
  );
  const clearFilters = () => {
    setStatusFilter("all");
    setProjectFilter(new Set<string>());
    setWorkgroupFilter(new Set<string>());
    setRoleFilter(new Set<string>());
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
    setExpandedGroupId((current) => (current === sessionId ? null : sessionId));
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
        setKillTarget(null);
      } else {
        setKillResult({
          sessionId: result.sessionId,
          state: result.state,
          message: result.message,
          blockedBySecurity: result.blockedBySecurity,
        });
      }
      await resourceMonitorStore.refresh();
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
            data-ac-testid="resourceMonitor.summary.appPrivateBytes"
            data-ac-role="metric"
          >
            <span class="rm-tile-label">App Private</span>
            <strong>{formatBytes(snapshot()?.appPrivateBytes)}</strong>
            <span
              class="rm-automation-metric"
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
          <div class="rm-banner rm-banner-muted">Resource monitoring is disabled.</div>
        </Show>

        <Show when={resourceMonitorStore.error}>
          <div class="rm-banner rm-banner-error">
            Snapshot failed: {resourceMonitorStore.error}
          </div>
        </Show>

        <Show when={resourceMonitorStore.stale && snapshot()}>
          <div class="rm-banner rm-banner-muted">
            Showing last snapshot from {formatTimestamp(snapshot()?.capturedAt)}.
          </div>
        </Show>

        <section class="rm-groups">
          <div class="rm-section-header">
            <h2>Agents</h2>
            <div class="rm-section-header-meta">
              <Show when={filtersActive()}>
                <span
                  class="rm-filter-count"
                  data-ac-testid="resourceMonitor.filter.count"
                  data-ac-role="text"
                >
                  Showing {filteredGroups().length} of {groups().length}
                </span>
              </Show>
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
                aria-label="Filter by workgroup"
                data-ac-testid="resourceMonitor.filter.workgroup"
                data-ac-role="group"
              >
                <span class="rm-filter-label">Workgroup</span>
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

          <Show
            when={filteredGroups().length > 0}
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
                    : "No agents match the filters"}
              </div>
            }
          >
            <div class="rm-group-list">
              <For each={filteredGroups()}>
                {(group) => (
                  <div
                    class={`rm-group-row state-${groupSeverity(group)}`}
                    data-ac-testid={`resourceMonitor.group.${group.sessionId}`}
                    data-ac-role="group"
                    data-ac-state={groupSeverity(group)}
                  >
                    <button
                      class="rm-group-main"
                      onClick={() => toggleGroup(group.sessionId)}
                      aria-expanded={expandedGroupId() === group.sessionId}
                      data-ac-testid={`resourceMonitor.group.${group.sessionId}.toggle`}
                      data-ac-role="button"
                    >
                      <span class="rm-expander">
                        {expandedGroupId() === group.sessionId ? "v" : ">"}
                      </span>
                      <span class="rm-group-identity">
                        <span
                          class="rm-group-name"
                          data-ac-testid={`resourceMonitor.group.${group.sessionId}.name`}
                          data-ac-role="cell"
                        >
                          {group.name}
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
                        class="rm-automation-metric"
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

                    <Show when={expandedGroupId() === group.sessionId}>
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
                              data-ac-testid={`resourceMonitor.group.${group.sessionId}.process.${process.pid}`}
                              data-ac-role="row"
                            >
                              <span
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
          <div class="rm-modal-backdrop" data-ac-testid="resourceMonitor.killConfirm">
            <div class="rm-modal" role="dialog" aria-modal="true">
              <h2>
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
                  class="rm-action-btn"
                  disabled={killInFlight()}
                  onClick={closeKillModal}
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
