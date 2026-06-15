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
import { ResourceMonitorAPI, SettingsAPI, WindowAPI, emitOpenSettings } from "../shared/ipc";
import { isTauri } from "../shared/platform";
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
  "quarantined",
  "failedCleanup",
  "unknownOwnership",
]);

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

const canKillGroup = (group: ResourceAgentGroupSnapshot): boolean =>
  group.killAllowed !== false && !NON_KILLABLE_GROUP_STATES.has(group.state);

const groupSeverity = (group: ResourceAgentGroupSnapshot): string => {
  if (group.state === "quarantined" || group.state === "failedCleanup") {
    return "critical";
  }
  if (group.state === "terminating") return "enforcing";
  if (group.state === "unknownOwnership" || group.lastError) return "warn";
  if (group.networkState === "unknown") return "unknown";
  return "ok";
};

const Titlebar: Component = () => {
  const handleDock = async () => {
    try {
      await WindowAPI.dockResourceMonitor();
    } catch (err) {
      console.error("Dock resource monitor failed:", err);
    }
  };

  const handleMinimize = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().minimize();
  };

  const handleClose = async () => {
    if (!isTauri) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
  };

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
            onClick={handleDock}
            title="Dock"
            aria-label="Dock Resource Monitor"
            data-ac-testid="resourceMonitor.titlebar.dock"
            data-ac-role="button"
          >
            <span
              class="rm-titlebar-btn-alias"
              data-ac-testid="resourceMonitor.dock"
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

const ResourceMonitorApp: Component = () => {
  const [expandedGroupId, setExpandedGroupId] = createSignal<string | null>(null);
  const [killTarget, setKillTarget] =
    createSignal<ResourceAgentGroupSnapshot | null>(null);
  const [killError, setKillError] = createSignal("");
  const [killInFlight, setKillInFlight] = createSignal(false);

  onMount(async () => {
    let resourcePreferences = DEFAULT_RESOURCE_PREFERENCES;
    document.documentElement.classList.add("light-theme");
    try {
      const settings = await SettingsAPI.get();
      resourcePreferences = {
        resourceBackoffPolling: settings.resourceBackoffPolling,
        resourceKeepLastSnapshot: settings.resourceKeepLastSnapshot,
      };
      if (!settings.themeLight) {
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
      await ResourceMonitorAPI.killGroup({
        sessionId: target.sessionId,
        reason: "user",
      });
      setKillTarget(null);
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
      <Titlebar />
      <main class="rm-body">
        <header class="rm-header">
          <div>
            <div class="rm-eyebrow">AgentsCommander</div>
            <h1>Resource Monitor</h1>
          </div>
          <div class="rm-header-actions">
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
            <span class="rm-tile-label">Active Groups</span>
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
            <h2>Agent Groups</h2>
            <span
              data-ac-testid="resourceMonitor.summary.timestamp"
              data-ac-role="text"
            >
              Last update {formatTimestamp(snapshot()?.capturedAt)}
            </span>
          </div>

          <Show
            when={groups().length > 0}
            fallback={
              <div
                class="rm-empty"
                data-ac-testid="resourceMonitor.empty"
                data-ac-role="status"
                data-ac-state={resourceMonitorStore.loading ? "loading" : "empty"}
              >
                {resourceMonitorStore.loading ? "Loading snapshot..." : "No active agent groups"}
              </div>
            }
          >
            <div class="rm-group-list">
              <For each={groups()}>
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
                      <span
                        class="rm-group-name"
                        data-ac-testid={`resourceMonitor.group.${group.sessionId}.name`}
                        data-ac-role="cell"
                      >
                        {group.name}
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
                      disabled={!canKillGroup(group)}
                      onClick={() => {
                        setKillError("");
                        setKillTarget(group);
                      }}
                      data-ac-testid={`resourceMonitor.group.${group.sessionId}.kill`}
                      data-ac-role="button"
                      data-ac-state={canKillGroup(group) ? "ready" : "disabled"}
                    >
                      Kill
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
              <h2>Kill agent group</h2>
              <p>{target.name}</p>
              <p class="rm-modal-detail">
                Session {target.sessionId} will be terminated by the backend resource watchdog.
              </p>
              <Show when={killError()}>
                <div class="rm-banner rm-banner-error">{killError()}</div>
              </Show>
              <div class="rm-modal-actions">
                <button
                  class="rm-action-btn"
                  disabled={killInFlight()}
                  onClick={() => setKillTarget(null)}
                  data-ac-testid="resourceMonitor.killConfirm.cancel"
                  data-ac-role="button"
                >
                  Cancel
                </button>
                <button
                  class="rm-action-btn rm-action-danger"
                  disabled={killInFlight()}
                  onClick={confirmKill}
                  data-ac-testid="resourceMonitor.killConfirm.confirm"
                  data-ac-role="button"
                >
                  {killInFlight() ? "Killing..." : "Kill Group"}
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
