import { Component, createSignal, createEffect, onCleanup, Show, onMount } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { UnlistenFn } from "../../shared/transport";
import { ProjectAPI, GuideAPI, SettingsAPI, SpecBoardAPI, emitThemeChanged, onOpenSettings } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { resourceMonitorStore } from "../../shared/stores/resourceMonitor";
import { setSoundsEnabled } from "../../shared/sound";
import { isBrowser } from "../../shared/platform";
import { homeStore } from "../../main/stores/home";
import { centralViewStore } from "../../main/stores/centralView";
import SettingsModal from "./SettingsModal";

const SELECTED_WORKGROUP_VISIBILITY_LABEL = "Always keep selected workgroup visible";

type ResourceBadgeState =
  | "disabled"
  | "unknown"
  | "ok"
  | "warn"
  | "critical"
  | "enforcing"
  | "limit";

const ActionBar: Component = () => {
  const [showDropdown, setShowDropdown] = createSignal(false);
  const [showSettings, setShowSettings] = createSignal(false);
  const [pendingSection, setPendingSection] = createSignal<string | undefined>(undefined, { equals: false });
  const [confirmPath, setConfirmPath] = createSignal<string | null>(null);
  const [toastMsg, setToastMsg] = createSignal<string | null>(null);
  const [isPendingDialog, setIsPendingDialog] = createSignal(false);
  const [showBrowserCreateProjectNotice, setShowBrowserCreateProjectNotice] = createSignal(false);
  const [localThemeLight, setLocalThemeLight] = createSignal(
    settingsStore.current?.themeLight ?? false,
  );
  createEffect(() => {
    const t = settingsStore.current?.themeLight;
    if (t !== undefined) setLocalThemeLight(t);
  });
  const isLight = (): boolean => localThemeLight();
  let dropdownRef: HTMLDivElement | undefined;

  const onClickAway = (e: MouseEvent) => {
    if (dropdownRef && !dropdownRef.contains(e.target as Node)) {
      setShowDropdown(false);
    }
  };

  createEffect(() => {
    if (showDropdown()) {
      document.addEventListener("mousedown", onClickAway);
    } else {
      document.removeEventListener("mousedown", onClickAway);
    }
  });

  onCleanup(() => document.removeEventListener("mousedown", onClickAway));

  const onConfirmKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") setConfirmPath(null);
  };

  createEffect(() => {
    if (confirmPath()) {
      window.addEventListener("keydown", onConfirmKeyDown);
    } else {
      window.removeEventListener("keydown", onConfirmKeyDown);
    }
  });

  onCleanup(() => window.removeEventListener("keydown", onConfirmKeyDown));

  let unlistenOpenSettings: UnlistenFn | null = null;
  let stopResourcePolling: (() => void) | null = null;
  onMount(async () => {
    unlistenOpenSettings = await onOpenSettings((section) => {
      setPendingSection(section);
      setShowSettings(true);
    });
    stopResourcePolling = resourceMonitorStore.startPolling({
      activeIntervalMs: 10_000,
      idleIntervalMs: 15_000,
      backoffIntervalMs: 20_000,
      backoffWhenIdle: settingsStore.current?.resourceBackoffPolling ?? true,
      keepLastSnapshot: settingsStore.current?.resourceKeepLastSnapshot ?? true,
    });
  });
  onCleanup(() => {
    if (unlistenOpenSettings) unlistenOpenSettings();
    if (stopResourcePolling) stopResourcePolling();
  });

  const handleNewProject = async () => {
    if (isBrowser) {
      setShowDropdown(false);
      setShowBrowserCreateProjectNotice(true);
      return;
    }
    if (isPendingDialog()) return;
    setShowDropdown(false);
    setIsPendingDialog(true);
    try {
      const { picked, hasAcRoot } = await projectStore.pickAndCheck();
      if (!picked) return;
      if (!hasAcRoot) {
        await projectStore.createAndLoad(picked);
      }
    } finally {
      setIsPendingDialog(false);
    }
  };

  const showToast = (msg: string) => {
    setToastMsg(msg);
    setTimeout(() => setToastMsg(null), 3000);
  };

  const handleOpenProject = async () => {
    if (isPendingDialog()) return;
    setShowDropdown(false);
    setIsPendingDialog(true);
    try {
      const picked = await open({ directory: true, title: "Select AC Project Folder" });
      if (!picked) return;
      const hasAcRoot = await ProjectAPI.checkPath(picked);
      if (hasAcRoot) {
        await projectStore.loadProject(picked);
      } else {
        showToast("No AC project found in this folder (.ac/ not found)");
      }
    } finally {
      setIsPendingDialog(false);
    }
  };

  const handleConfirmCreate = async () => {
    const path = confirmPath();
    if (path) {
      await projectStore.createAndLoad(path);
      setConfirmPath(null);
    }
  };

  const isSoundsEnabled = (): boolean =>
    settingsStore.current?.soundsEnabled ?? true;
  const handleToggleMute = async () => {
    const previousValue = isSoundsEnabled();
    const newValue = !previousValue;
    setSoundsEnabled(newValue);
    try {
      await SettingsAPI.setSoundsEnabled(newValue);
      settingsStore.refresh();
    } catch (err) {
      setSoundsEnabled(previousValue);
      const msg = err instanceof Error ? err.message : String(err);
      showToast(`Failed to ${newValue ? "unmute" : "mute"} sounds: ${msg}`);
    }
  };

  const applyThemeClass = (light: boolean) => {
    if (light) {
      document.documentElement.classList.add("light-theme");
    } else {
      document.documentElement.classList.remove("light-theme");
    }
  };
  const handleToggleTheme = async () => {
    const previousValue = isLight();
    const newValue = !previousValue;
    setLocalThemeLight(newValue);
    applyThemeClass(newValue);
    emitThemeChanged(newValue).catch(console.error);
    try {
      await SettingsAPI.setThemeLight(newValue);
      settingsStore.refresh();
    } catch (err) {
      setLocalThemeLight(previousValue);
      applyThemeClass(previousValue);
      emitThemeChanged(previousValue).catch(console.error);
      const msg = err instanceof Error ? err.message : String(err);
      showToast(`Failed to switch to ${newValue ? "light" : "dark"} theme: ${msg}`);
    }
  };

  const resourceBadgeState = (): ResourceBadgeState => {
    if (settingsStore.current?.resourceMonitorEnabled === false) return "disabled";

    const snapshot = resourceMonitorStore.snapshot;
    if (!snapshot || resourceMonitorStore.error) return "unknown";
    if (snapshot.overallState === "critical" || snapshot.overallState === "enforcing") {
      return snapshot.overallState;
    }
    if (snapshot.overallState === "warn") return "warn";
    if (
      snapshot.maxConcurrentAgentGroups > 0 &&
      snapshot.activeAgentGroups >= snapshot.maxConcurrentAgentGroups
    ) {
      return "limit";
    }
    if (snapshot.overallState === "unknown" || snapshot.networkState === "unknown") {
      return "unknown";
    }
    return "ok";
  };

  const resourceBadgeTitle = (): string => {
    const state = resourceBadgeState();
    const snapshot = resourceMonitorStore.snapshot;
    if (state === "disabled") return "Resource Monitor disabled";
    if (!snapshot) return "Resource Monitor status unknown";
    return `Resource Monitor: ${snapshot.activeAgentGroups}/${snapshot.maxConcurrentAgentGroups} agents, ${state}`;
  };

  const handleToggleResourceMonitor = () => {
    centralViewStore.toggleResourceMonitor();
  };

  return (
    <>
      <div
        class="action-bar"
        data-ac-testid="actionBar"
        data-ac-role="surface"
      >
        <div class="action-bar-dropdown" ref={dropdownRef}>
          <button
            class="action-bar-dropdown-btn"
            disabled={isPendingDialog()}
            onClick={() => setShowDropdown(!showDropdown())}
            aria-expanded={showDropdown()}
            data-ac-testid="actionBar.newOpen"
            data-ac-role="button"
            data-ac-state={isPendingDialog() ? "disabled" : showDropdown() ? "open" : "closed"}
          >
            New / Open
            <svg class="action-bar-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
          <Show when={showDropdown()}>
            <div
              class="action-bar-menu"
              data-ac-testid="actionBar.menu"
              data-ac-role="menu"
            >
              <button
                class="action-bar-menu-item"
                disabled={isPendingDialog()}
                onClick={handleNewProject}
                data-ac-testid="actionBar.menu.newProject"
                data-ac-role="menuitem"
                data-ac-state={isPendingDialog() ? "disabled" : "ready"}
              >
                &#x1F4C1; New Project
              </button>
              <button
                class="action-bar-menu-item"
                disabled={isPendingDialog()}
                onClick={handleOpenProject}
                data-ac-testid="actionBar.menu.openProject"
                data-ac-role="menuitem"
                data-ac-state={isPendingDialog() ? "disabled" : "ready"}
              >
                &#x1F4C2; Open Project
              </button>
            </div>
          </Show>
        </div>
        <div class="action-bar-icons">
          <Show when={settingsStore.current?.specBoardEnabled === true}>
            <button
              class="toolbar-gear-btn"
              onClick={() => SpecBoardAPI.open()}
              title="Spec board"
              data-ac-testid="actionBar.specBoard"
              data-ac-role="button"
            >
              &#x25A7;
            </button>
          </Show>

          <button
            class={`toolbar-gear-btn home-toggle-btn ${homeStore.visible ? "active" : ""}`}
            onClick={() => homeStore.toggle()}
            title={homeStore.visible ? "Hide Home" : "Show Home"}
            aria-label={homeStore.visible ? "Hide Home" : "Show Home"}
            aria-pressed={homeStore.visible}
            data-ac-testid="actionBar.home"
            data-ac-role="button"
            data-ac-state={homeStore.visible ? "visible" : "hidden"}
          >
            &#x1F3E0;
          </button>
          <button
            class={`toolbar-gear-btn coord-sort-activity-btn ${sessionsStore.coordSortByActivity ? "active" : ""}`}
            disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}
            onClick={() => sessionsStore.toggleCoordSortByActivity()}
            title={sessionsStore.coordSortByActivity ? "Show recent coordinators first" : "Show coordinators in default order"}
            aria-pressed={sessionsStore.coordSortByActivity}
            data-ac-testid="actionBar.sortCoordinators"
            data-ac-role="button"
            data-ac-state={!sessionsStore.hydrated || sessionsStore.toggleInFlight ? "disabled" : sessionsStore.coordSortByActivity ? "recent" : "default"}
          >
            &#x1F525;
          </button>
          <button
            class={`toolbar-gear-btn sounds-mute-btn ${isSoundsEnabled() ? "" : "active"}`}
            disabled={!settingsStore.current}
            onClick={handleToggleMute}
            title={isSoundsEnabled() ? "Mute all app sounds" : "Unmute app sounds"}
            aria-label={isSoundsEnabled() ? "Mute all app sounds" : "Unmute app sounds"}
            aria-pressed={!isSoundsEnabled()}
            data-ac-testid="actionBar.sounds"
            data-ac-role="button"
            data-ac-state={!settingsStore.current ? "disabled" : isSoundsEnabled() ? "audible" : "muted"}
          >
            {isSoundsEnabled() ? "🔊" : "🔇"}
          </button>
          <button
            class={`toolbar-gear-btn show-categories-btn ${sessionsStore.showCategories ? "active" : ""}`}
            onClick={() => sessionsStore.toggleShowCategories()}
            title={sessionsStore.showCategories ? "Hide category sections" : "Show category sections"}
            aria-pressed={sessionsStore.showCategories}
            data-ac-testid="actionBar.categories"
            data-ac-role="button"
            data-ac-state={sessionsStore.showCategories ? "visible" : "hidden"}
          >
            &#x1F441;
          </button>
          <button
            class={`toolbar-gear-btn show-categories-btn ${sessionsStore.alwaysShowSelectedWorkgroup ? "active" : ""}`}
            onClick={() => sessionsStore.toggleAlwaysShowSelectedWorkgroup()}
            title={SELECTED_WORKGROUP_VISIBILITY_LABEL}
            aria-label={SELECTED_WORKGROUP_VISIBILITY_LABEL}
            aria-pressed={sessionsStore.alwaysShowSelectedWorkgroup}
            data-ac-testid="actionBar.pinSelectedWorkgroup"
            data-ac-role="button"
            data-ac-state={sessionsStore.alwaysShowSelectedWorkgroup ? "pinned" : "default"}
          >
            &#x1F4CC;
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => GuideAPI.open()}
            title="Hints"
            data-ac-testid="actionBar.guide"
            data-ac-role="button"
          >
            &#x1F4A1;
          </button>
          <button
            class={`toolbar-gear-btn resource-monitor-btn state-${resourceBadgeState()} ${centralViewStore.isResourceMonitor ? "active" : ""}`}
            onClick={handleToggleResourceMonitor}
            title={resourceBadgeTitle()}
            aria-label={resourceBadgeTitle()}
            aria-pressed={centralViewStore.isResourceMonitor}
            data-ac-testid="actionBar.resourceMonitor"
            data-ac-role="button"
            data-ac-state={resourceBadgeState()}
          >
            <span class="resource-monitor-glyph" aria-hidden="true">&#x25A6;</span>
          </button>
          <button
            class="toolbar-gear-btn"
            disabled={!settingsStore.current}
            onClick={handleToggleTheme}
            title="Toggle theme"
            data-ac-testid="actionBar.theme"
            data-ac-role="button"
            data-ac-state={!settingsStore.current ? "disabled" : isLight() ? "light" : "dark"}
          >
            {isLight() ? "\u2600\uFE0F" : "\uD83C\uDF19"}
          </button>
          <button
            class="toolbar-gear-btn"
            onClick={() => { setPendingSection(undefined); setShowSettings(true); }}
            title="Settings"
            data-ac-testid="actionBar.settings"
            data-ac-role="button"
          >
            &#x2699;
          </button>
        </div>
      </div>

      {showSettings() && (
        <SettingsModal
          onClose={() => setShowSettings(false)}
          section={pendingSection()}
        />
      )}
      <Show when={showBrowserCreateProjectNotice()}>
        <div
          class="modal-overlay"
          data-ac-testid="project.browserCreateNotice.overlay"
          data-ac-role="overlay"
        >
          <div
            class="agent-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="projectBrowserCreateNoticeTitle"
            style={{ "max-width": "380px" }}
            onClick={(e) => e.stopPropagation()}
            data-ac-testid="project.browserCreateNotice.dialog"
            data-ac-role="dialog"
          >
            <div class="agent-modal-header">
              <span id="projectBrowserCreateNoticeTitle" class="agent-modal-title">
                Create a project from the desktop app
              </span>
            </div>
            <div class="new-agent-form">
              <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                Creating a new project isn't available in the browser view. Open the
                AgentsCommander desktop app to create a project — it will then appear here.
              </p>
              <div class="new-agent-footer">
                <button
                  class="new-agent-create-btn"
                  onClick={() => setShowBrowserCreateProjectNotice(false)}
                  data-ac-testid="project.browserCreateNotice.dismiss"
                  data-ac-role="button"
                >
                  Got it
                </button>
              </div>
            </div>
          </div>
        </div>
      </Show>
      <Show when={confirmPath()}>
        <div
          class="confirm-overlay"
          data-ac-testid="project.createConfirm.overlay"
          data-ac-role="overlay"
        >
          <div
            class="confirm-dialog"
            onClick={(e) => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            data-ac-testid="project.createConfirm.dialog"
            data-ac-role="dialog"
          >
            <p class="confirm-text">
              This folder does not have an AC project. Do you want to create a new project here?
            </p>
            <p class="confirm-path">{confirmPath()}</p>
            <div class="confirm-actions">
              <button
                class="confirm-btn confirm-btn-yes"
                onClick={handleConfirmCreate}
                data-ac-testid="project.createConfirm.confirm"
                data-ac-role="button"
              >
                Yes, create project
              </button>
              <button
                class="confirm-btn confirm-btn-no"
                onClick={() => setConfirmPath(null)}
                data-ac-testid="project.createConfirm.cancel"
                data-ac-role="button"
              >
                Cancel
              </button>
            </div>
          </div>
        </div>
      </Show>
      <Show when={toastMsg()}>
        <div class="toast-error">{toastMsg()}</div>
      </Show>
    </>
  );
};

export default ActionBar;
