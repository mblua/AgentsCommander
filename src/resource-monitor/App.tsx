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

// #647 C: a `quarantined` group is now killable again — the Force-kill / Retry
// affordance re-fires the durable Job Object kill (it was wrongly disabled,
// stranding agents the AV kept alive). `terminating` stays non-killable (a kill
// is already in flight); `terminated` is done; `failedCleanup` /
// `unknownOwnership` are never emitted by the 4-variant backend state.
const NON_KILLABLE_GROUP_STATES = new Set<ResourceGroupState>([
  "terminating",
  "terminated",
  "failedCleanup",
  "unknownOwnership",
]);

// #647 C: states where the backend is still working the kill (now re-sampled
// every snapshot), so the row shows a "Verifying..." indicator.
const VERIFYING_GROUP_STATES = new Set<ResourceGroupState>([
  "terminating",
  "quarantined",
]);

// #647 D: English guidance shown (ADDED to, never replacing, the per-PID detail)
// when a kill is blocked by a security product stripping PROCESS_TERMINATE.
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

// #516 - "wg-5-dev-team / dev-rust" identity so the user can tell which group a
// Kill targets. Falls back to the coding-agent name when no replica identity is
// present (non-WG / ad-hoc launches), and "-" for an unknown workgroup.
const groupOrigin = (group: ResourceAgentGroupSnapshot): string =>
  `${group.workgroup ?? "-"} / ${group.agent ?? group.name}`;

const canKillGroup = (group: ResourceAgentGroupSnapshot): boolean =>
  group.killAllowed !== false && !NON_KILLABLE_GROUP_STATES.has(group.state);

// #647 C: a quarantined group is killable but its button re-fires the durable
// kill, so it reads "Force-kill" rather than "Kill".
const killActionLabel = (group: ResourceAgentGroupSnapshot): string =>
  group.state === "quarantined" ? "Force-kill" : "Kill";

const isVerifying = (group: ResourceAgentGroupSnapshot): boolean =>
  VERIFYING_GROUP_STATES.has(group.state);

const groupSeverity = (group: ResourceAgentGroupSnapshot): string => {
  if (group.state === "quarantined" || group.state === "failedCleanup") {
    return "critical";
  }
  if (group.state === "terminating") return "enforcing";
  if (group.state === "unknownOwnership" || group.lastError) return "warn";
  if (group.networkState === "unknown") return "unknown";
  return "ok";
};

// #566 - active = the agent still holds (or is releasing) live OS processes /
// a concurrency slot: every state except `terminated` (matches the backend's
// active_count semantics). Single source of truth for the status filter (A.1).
const isActiveGroup = (group: ResourceAgentGroupSnapshot): boolean =>
  group.state !== "terminated";

// #566 - status filter is component-local view state, not part of the IPC data
// contract (deliberately kept out of shared/types.ts).
type RmStatusFilter = "all" | "active" | "inactive";

const STATUS_FILTERS: ReadonlyArray<{ value: RmStatusFilter; label: string }> = [
  { value: "all", label: "All" },
  { value: "active", label: "Active" },
  { value: "inactive", label: "Inactive" },
];

// #566 - distinct, sorted, non-empty values for a filter dimension. Derives from
// the full snapshot set so a value stays selectable even while filtered out.
const distinct = (values: (string | null | undefined)[]): string[] =>
  [...new Set(values.filter((v): v is string => !!v))].sort();

// #566 - G3 reactivity contract: a multi-select toggle MUST replace the Set with
// a fresh copy. Mutating in place returns the same reference, so SolidJS's
// Object.is equality treats it as unchanged and the dependent memos silently
// never re-run (the filters render, clicks register, nothing filters).
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
  // #587 - tracks the real OS maximize state so the button glyph (maximize vs
  // restore) and its labels stay correct. Not all maximize paths route through
  // handleMaximize: Tauri v2 also maximizes natively on data-tauri-drag-region
  // double-click, and the OS does it on Win+Up / edge-snap. onResized below
  // re-reads the truth after every such change.
  const [maximized, setMaximized] = createSignal(false);

  // #587 — "Attach" pulls RM back into the main window's central pane (replaces
  // the old window-snapping "Dock"). Emit BEFORE closing so the event is queued
  // before this webview tears down; main's listener runs showResourceMonitor().
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
      // Use explicit maximize()/unmaximize() rather than toggleMaximize().
      // The app capability set (src-tauri/capabilities/default.json) grants
      // core:window:allow-maximize, allow-unmaximize and allow-is-maximized,
      // but NOT allow-toggle-maximize. toggleMaximize() therefore gets rejected
      // at the Tauri v2 ACL layer at runtime, which is why the button "did
      // nothing" in the real build (the rejection was only logged below). The
      // drag-region double-click still maximizes because that path uses the
      // separate allow-internal-toggle-maximize permission, which IS in
      // core:window:default.
      if (await win.isMaximized()) {
        await win.unmaximize();
      } else {
        await win.maximize();
      }
      // Reflect immediately so the icon swaps without waiting for the resize
      // event. On failure leave the glyph as-is; the onResized mirror reconciles
      // it once the window settles.
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
    // Register cleanup synchronously so it binds to this component's owner; the
    // async listener registration below fills in `unlisten` once it resolves.
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
  /** True when mounted inside MainApp's central pane. Suppresses the window
   *  titlebar and skips documentElement theme init (main owns it), and shows a
   *  "Detach" button that pops RM out into its own window (#587). */
  embedded?: boolean;
}

const ResourceMonitorApp: Component<ResourceMonitorAppProps> = (props) => {
  const [expandedGroupId, setExpandedGroupId] = createSignal<string | null>(null);
  const [killTarget, setKillTarget] =
    createSignal<ResourceAgentGroupSnapshot | null>(null);
  const [killError, setKillError] = createSignal("");
  const [killInFlight, setKillInFlight] = createSignal(false);
  // #647 D: the outcome of the last kill attempt that quarantined (verified-dead
  // kills clear it). Keyed by sessionId so the per-PID detail + security hint
  // attach to the right row and modal. Held in the FE because blockedBySecurity
  // is a per-attempt result flag, not part of the persisted group snapshot.
  const [killResult, setKillResult] = createSignal<{
    sessionId: string;
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

  // #587 — pop the embedded RM out into its own OS window, then reveal the
  // terminal in the pane (preserves the embedded-XOR-detached invariant).
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
    // #587 — embedded RM inherits the theme MainApp owns on documentElement; it
    // must not add/remove the class itself (would fight main). Windowed RM still
    // does its own optimistic light-first paint + corrective read.
    if (!props.embedded) document.documentElement.classList.add("light-theme");
    try {
      const settings = await SettingsAPI.get();
      resourcePreferences = {
        resourceBackoffPolling: settings.resourceBackoffPolling,
        resourceKeepLastSnapshot: settings.resourceKeepLastSnapshot,
      };
      if (!props.embedded && !settings.themeLight) {
        document.documentElement.classList.remove("light-theme");
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

  // #566 - filter view-state (ephemeral, not persisted). The three multi-selects
  // are Set<string>; every toggle goes through toggleFilter (fresh-Set copy) so
  // the dependent memos actually re-run (see the G3 contract above).
  const [statusFilter, setStatusFilter] = createSignal<RmStatusFilter>("all");
  const [projectFilter, setProjectFilter] = createSignal<Set<string>>(new Set());
  const [workgroupFilter, setWorkgroupFilter] = createSignal<Set<string>>(
    new Set()
  );
  const [roleFilter, setRoleFilter] = createSignal<Set<string>>(new Set());

  // Option lists derive from the FULL snapshot set, not the filtered one, so a
  // value stays selectable even while it is filtered out (A.4.2).
  const projectOptions = createMemo(() =>
    distinct(groups().map((g) => g.project))
  );
  const workgroupOptions = createMemo(() =>
    distinct(groups().map((g) => g.workgroup))
  );
  const roleOptions = createMemo(() => distinct(groups().map((g) => g.agent)));

  // AND across dimensions, OR within a dimension; an empty Set = no constraint.
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
      if (result.quarantined) {
        // #647 A: NOT verified dead — the agent may still be alive. Keep the
        // modal open so the user can read the per-PID detail (+ AV-exclusion
        // hint when blocked) and Retry; the durable job is preserved backend-side.
        setKillResult({
          sessionId: result.sessionId,
          message: result.message,
          blockedBySecurity: result.blockedBySecurity,
        });
      } else {
        // #647 A: verified dead — the backend emitted session_created carrying
        // the Exited SessionInfo (the sidebar tile flips via the addSession
        // upsert, E5). Close the modal and refresh the snapshot.
        setKillResult(null);
        setKillTarget(null);
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
                  // #566 N1 - loading only wins when there is genuinely no data
                  // yet (groups().length === 0). With keepLastSnapshot, every
                  // poll's setLoading(true) would otherwise flash "loading" over
                  // an already-populated-but-filtered view each cycle. Once rows
                  // exist but the active filter matches none, stay on the stable
                  // filtered-empty state; initial load (no data) still shows it.
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
              {/* #647 D: a quarantined (not-verified-dead) result keeps the modal
                  open and shows the per-PID detail; the AV-exclusion hint is
                  PREPENDED above it when the kill was blocked by security. */}
              <Show when={killResult()} keyed>
                {(res) => (
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
                )}
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
