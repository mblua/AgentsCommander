import { Component, createSignal, createEffect, createMemo, on, onMount, onCleanup, Show } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import type {
  SessionStatus,
  ContextTemplateUpdate,
  MainSidebarSide,
  SessionWarning,
} from "../shared/types";
import {
  PtyAPI,
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  ReposAPI,
  WindowAPI,
  ProjectAPI,
  onSessionCreated,
  onSessionDestroyed,
  onSessionSwitched,
  onSessionRenamed,
  onSessionCommunicationChanged,
  onSessionIdle,
  onSessionBusy,
  onSessionContext,
  onSessionGitRepos,
  onSessionCoordinatorChanged,
  onTelegramBridgeAttached,
  onTelegramBridgeDetached,
  onTelegramBridgeError,
  onSessionEnvWarning,
  onTerminalDetached,
  onTerminalAttached,
  onWorkgroupTaskUpdated,
  onAcProjectRefreshRequested,
  onCodingAgentProfilesUpdated,
  onCodingAgentEnvSettingsUpdated,
  onCodingAgentProfileSelectionUpdated,
  onLoopEvent,
  onProjectGroupsUpdated,
  onProjectArchiveChanged,
  onNpmUpdateAvailable,
} from "../shared/ipc";
import { taskFirstLine } from "../shared/markdown";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { applyWindowLayout } from "../shared/window-layout";
import { sessionsStore } from "./stores/sessions";
import { bridgesStore } from "./stores/bridges";
import { projectStore } from "./stores/project";
import { workgroupGroupsStore } from "./stores/workgroup-groups";
import { normalizeProjectPathForCompare } from "./stores/project-refresh";
import { startTeamIdleWatcher } from "./stores/team-idle-watcher";
import { primeAudio } from "../shared/sound";
import { settingsStore } from "../shared/stores/settings";
import { railCollapseStore } from "./stores/rail-collapse";
import Titlebar from "./components/Titlebar";
import ActionBar from "./components/ActionBar";
import RootAgentBanner from "./components/RootAgentBanner";
import ProjectPanel from "./components/ProjectPanel";
import WorkgroupGroupRail from "./components/WorkgroupGroupRail";
import OnboardingModal from "./components/OnboardingModal";
import ContextTemplateUpdateModal from "./components/ContextTemplateUpdateModal";
import AutoUnarchiveModal from "./components/AutoUnarchiveModal";
import ToastHost from "../shared/components/ToastHost";
import { toastStore } from "../shared/stores/toasts";
import { autoUnarchiveStore } from "./stores/auto-unarchive";
import { handleProjectRefreshRequested } from "./project-refresh-handler";
import { loopToastFromEvent, type LoopToast } from "./loop-event-toast";
import { createUpdateToaster } from "./update-toast";
import { wireScreenshotListeners } from "./listeners-screenshot";
import "./styles/sidebar.css";
import "../shared/styles/toast.css";

interface SidebarAppProps {
  embedded?: boolean;
  railSide?: MainSidebarSide;
}

function isExitedStatus(status: SessionStatus): boolean {
  return typeof status === "object" && status !== null && "exited" in status;
}

function sessionWarningIdentity(warning: SessionWarning): string {
  return JSON.stringify([
    warning.sessionId,
    warning.key,
    warning.kind,
    warning.message,
  ]);
}

function incrementWarningCount(counts: Map<string, number>, warning: SessionWarning): void {
  const key = sessionWarningIdentity(warning);
  counts.set(key, (counts.get(key) ?? 0) + 1);
}

function consumeWarningCount(counts: Map<string, number>, warning: SessionWarning): boolean {
  const key = sessionWarningIdentity(warning);
  const count = counts.get(key) ?? 0;
  if (count <= 0) return false;
  if (count === 1) {
    counts.delete(key);
  } else {
    counts.set(key, count - 1);
  }
  return true;
}

export function blockContextMenu(e: Event): void {
  if (e.target instanceof Element && e.target.closest(".terminal-host, .project-filter-row")) return;
  e.preventDefault();
}

export function activeWorkgroupGroupSelectionKey(): string | null {
  const projectPath = workgroupGroupsStore.activeProjectPath();
  if (!projectPath) return null;
  const selection = workgroupGroupsStore.selection(projectPath);
  return JSON.stringify([
    projectPath,
    selection.kind,
    selection.kind === "group" ? selection.id : null,
  ]);
}

function projectPanelForPath(
  scrollContainer: HTMLElement,
  normalizedProjectPath: string,
): HTMLElement | null {
  const header = Array.from(
    scrollContainer.querySelectorAll<HTMLElement>(".project-header"),
  ).find((candidate) =>
    normalizeProjectPathForCompare(candidate.getAttribute("title") ?? "") === normalizedProjectPath
  );
  return header?.closest<HTMLElement>(".project-panel") ?? null;
}

export function createSidebarSelectionScrollReset(
  scrollContainer: () => HTMLDivElement | undefined,
): void {
  const selectionKey = createMemo(activeWorkgroupGroupSelectionKey);
  let disposed = false;

  createEffect(on(selectionKey, (key) => {
    if (!key) return;
    const projectPath = workgroupGroupsStore.activeProjectPath();
    if (!projectPath) return;

    queueMicrotask(() => {
      if (disposed || selectionKey() !== key) return;
      const container = scrollContainer();
      if (!container) return;

      const projectPanel = projectPanelForPath(container, projectPath);
      if (projectPanel) {
        const containerTop = container.getBoundingClientRect().top;
        const projectTop = projectPanel.getBoundingClientRect().top;
        container.scrollTop = Math.max(0, container.scrollTop + projectTop - containerTop);
      }
    });
  }, { defer: true }));

  onCleanup(() => {
    disposed = true;
  });
}

const SidebarApp: Component<SidebarAppProps> = (props) => {
  const [showOnboarding, setShowOnboarding] = createSignal(false);
  const [loopToast, setLoopToast] = createSignal<LoopToast | null>(null);
  const [settingsRailSide, setSettingsRailSide] = createSignal<MainSidebarSide>("right");
  const [activeContextTemplateUpdate, setActiveContextTemplateUpdate] =
    createSignal<ContextTemplateUpdate | null>(null);
  const [contextTemplateUpdateBusy, setContextTemplateUpdateBusy] = createSignal(false);
  const [contextTemplateUpdateError, setContextTemplateUpdateError] =
    createSignal<string | null>(null);
  const seenContextTemplateUpdates = new Set<string>();
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let stopTeamIdleWatcher: (() => void) | null = null;
  let loopToastTimer: ReturnType<typeof setTimeout> | null = null;
  let raiseTerminalEnabled = true;
  let lastRaiseTime = 0;
  const railSide = () => props.railSide ?? settingsRailSide();
  let sidebarScrollableEl: HTMLDivElement | undefined;
  createSidebarSelectionScrollReset(() => sidebarScrollableEl);
  let profileDriftRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  const handleMainSidebarSideChange = (event: Event) => {
    const side = (event as CustomEvent<{ side?: MainSidebarSide }>).detail?.side;
    setSettingsRailSide(side === "left" ? "left" : "right");
  };

  const handleRaiseTerminal = async (e: MouseEvent) => {
    if (!isTauri || props.embedded || !raiseTerminalEnabled) return;
    const tag = (e.target as HTMLElement).tagName;
    if (tag === "SELECT" || tag === "INPUT" || tag === "TEXTAREA" || tag === "BUTTON") return;
    const now = Date.now();
    if (now - lastRaiseTime < 500) return;
    lastRaiseTime = now;
    try {
      await WindowAPI.ensureTerminal();
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setFocus();
    } catch {}
  };

  const showLoopToast = (toast: LoopToast) => {
    if (loopToastTimer) clearTimeout(loopToastTimer);
    setLoopToast(toast);
    loopToastTimer = setTimeout(() => {
      setLoopToast(null);
      loopToastTimer = null;
    }, 3000);
  };

  const showUpdateToast = createUpdateToaster();

  const contextTemplateUpdateKey = (update: ContextTemplateUpdate) =>
    `${update.projectPath}\n${update.filename}\n${update.currentDefaultSha256}\n${update.currentFileSha256}`;

  const nextContextTemplateUpdate = (): ContextTemplateUpdate | null => {
    for (const project of projectStore.projects) {
      for (const update of project.contextTemplateUpdates) {
        if (!seenContextTemplateUpdates.has(contextTemplateUpdateKey(update))) {
          return update;
        }
      }
    }
    return null;
  };

  const formatContextTemplateError = (e: unknown): string => {
    if (typeof e === "string") return e;
    if (e instanceof Error) return e.message;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  };

  const resolveContextTemplateUpdate = (update: ContextTemplateUpdate) => {
    projectStore.removeContextTemplateUpdate(
      update.projectPath,
      update.filename,
      update.currentDefaultSha256,
      update.currentFileSha256
    );
    setActiveContextTemplateUpdate(null);
  };

  const keepContextTemplateUpdate = async () => {
    const update = activeContextTemplateUpdate();
    if (!update) return;
    setContextTemplateUpdateBusy(true);
    setContextTemplateUpdateError(null);
    try {
      await ProjectAPI.keepCustomContextTemplate(update);
      resolveContextTemplateUpdate(update);
    } catch (e) {
      setContextTemplateUpdateError(formatContextTemplateError(e));
    } finally {
      setContextTemplateUpdateBusy(false);
    }
  };

  const overwriteContextTemplateUpdate = async () => {
    const update = activeContextTemplateUpdate();
    if (!update) return;
    setContextTemplateUpdateBusy(true);
    setContextTemplateUpdateError(null);
    try {
      const result = await ProjectAPI.overwriteContextTemplateWithDefault(update);
      resolveContextTemplateUpdate(update);
      toastStore.info(
        `${update.label} overwritten with the new default. Your previous version was saved to ${result.backupPath}`,
        { durationMs: null }
      );
    } catch (e) {
      setContextTemplateUpdateError(formatContextTemplateError(e));
    } finally {
      setContextTemplateUpdateBusy(false);
    }
  };

  createEffect(() => {
    if (activeContextTemplateUpdate()) return;
    const next = nextContextTemplateUpdate();
    if (!next) return;
    seenContextTemplateUpdates.add(contextTemplateUpdateKey(next));
    setContextTemplateUpdateError(null);
    setActiveContextTemplateUpdate(next);
  });

  const refreshProfileOutdated = async () => {
    try {
      const list = await SessionAPI.list();
      for (const s of list) {
        sessionsStore.setProfileOutdated(s.id, s.profileOutdated ?? false);
      }
    } catch (e) {
      console.error("Failed to refresh profile drift:", e);
    }
  };
  const scheduleProfileOutdatedRefresh = () => {
    if (profileDriftRefreshTimer) clearTimeout(profileDriftRefreshTimer);
    profileDriftRefreshTimer = setTimeout(() => {
      profileDriftRefreshTimer = null;
      void refreshProfileOutdated();
    }, 250);
  };
  const handleWindowFocusDriftRefresh = () => {
    if (document.visibilityState === "hidden") return;
    scheduleProfileOutdatedRefresh();
  };

  const liveWarningsDuringInitialDrain = new Map<string, number>();
  let initialWarningDrainActive = false;
  let disposed = false;

  const surfaceSessionWarning = (warning: SessionWarning) => {
    if (disposed) return;
    console.warn(
      `Session warning for ${warning.sessionId} (${warning.key}/${warning.kind}): ${warning.message}`
    );
    toastStore.error(warning.message);
  };

  const handleLiveSessionWarning = (warning: SessionWarning) => {
    if (initialWarningDrainActive) {
      incrementWarningCount(liveWarningsDuringInitialDrain, warning);
    }
    surfaceSessionWarning(warning);
  };

  const drainBufferedSessionWarnings = async () => {
    initialWarningDrainActive = true;
    try {
      const warnings = await SessionAPI.drainWarnings();
      for (const warning of warnings) {
        if (consumeWarningCount(liveWarningsDuringInitialDrain, warning)) continue;
        surfaceSessionWarning(warning);
      }
    } catch (e) {
      console.error("[session-warning] drain_session_warnings failed:", e);
    } finally {
      initialWarningDrainActive = false;
      liveWarningsDuringInitialDrain.clear();
    }
  };

  onMount(async () => {
    unlisteners.push(await onSessionEnvWarning(handleLiveSessionWarning));
    void drainBufferedSessionWarnings();

    unlisteners.push(
      await onAcProjectRefreshRequested((data) => {
        handleProjectRefreshRequested(data);
      })
    );
    unlisteners.push(
      await onProjectGroupsUpdated((data) => {
        workgroupGroupsStore.applyExternalUpdate(data.projectPath, data.config);
      })
    );
    unlisteners.push(
      await onProjectArchiveChanged((data) => {
        void projectStore.applyArchiveChange(data).catch((error) => {
          console.error("[archive] Failed to apply project_archive_changed:", error);
        });
        if (data.reason === "autoUnarchive") {
          autoUnarchiveStore.push(data);
        }
      })
    );
    unlisteners.push(
      await onCodingAgentProfilesUpdated(() => {
        settingsStore.refresh();
        void refreshProfileOutdated();
      })
    );
    unlisteners.push(
      await onNpmUpdateAvailable((info) => showUpdateToast(info))
    );
    void SettingsAPI.getUpdateStatus()
      .then((pending) => {
        if (pending) showUpdateToast(pending);
      })
      .catch((err) => {
        console.error("[update-check] getUpdateStatus failed:", err);
      });
    unlisteners.push(...(await wireScreenshotListeners()));
    unlisteners.push(
      await onCodingAgentEnvSettingsUpdated(() => {
        settingsStore.refresh();
        void refreshProfileOutdated();
      })
    );
    unlisteners.push(
      await onCodingAgentProfileSelectionUpdated((data) => {
        settingsStore.refresh();
        void refreshProfileOutdated();
        if (data.agentPath) {
          void projectStore.reloadProjectIfLoaded(data.agentPath);
        } else {
          for (const proj of projectStore.projects) {
            void projectStore.reloadProject(proj.path);
          }
        }
      })
    );
    unlisteners.push(
      await onLoopEvent((data) => {
        if (data.summary) {
          projectStore.upsertLoop(data.projectPath, data.summary);
        } else if (data.kind === "deleted") {
          projectStore.removeLoop(data.projectPath, data.loopId);
        }
        void projectStore.reloadProjectIfLoaded(data.projectPath);
        const toast = loopToastFromEvent(data);
        if (toast) showLoopToast(toast);
      })
    );

    shortcutHandler = registerShortcuts();
    if (!props.embedded) {
      cleanupZoom = await initZoom("sidebar");
      cleanupGeometry = await initWindowGeometry("sidebar");
    }

    const appSettings = await SettingsAPI.get();
    setSettingsRailSide(appSettings.mainSidebarSide === "left" ? "left" : "right");
    if (!props.embedded) {
      document.documentElement.classList.toggle("light-theme", appSettings.themeLight);
    }
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    sessionsStore.setCoordSortByActivity(appSettings.coordSortByActivity ?? false);
    sessionsStore.setAlwaysShowSelectedWorkgroup(appSettings.alwaysShowSelectedWorkgroup ?? true);
    railCollapseStore.hydrateFromSettings(appSettings);
    const style = appSettings.sidebarStyle;
    const removedThemes = ["classic", "signal-grid"];
    document.documentElement.dataset.sidebarStyle = (!style || removedThemes.includes(style)) ? "noir-minimal" : style;
    if (!props.embedded && appSettings.sidebarAlwaysOnTop && isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(true);
    }
    if (!props.embedded) {
      document.addEventListener("mousedown", handleRaiseTerminal);
    }

    document.addEventListener("contextmenu", blockContextMenu);
    window.addEventListener("main-sidebar-side-change", handleMainSidebarSideChange);

    if (!props.embedded) {
      try {
        await applyWindowLayout("right");
      } catch {}
    }

    await settingsStore.load();

    primeAudio();
    stopTeamIdleWatcher = startTeamIdleWatcher();

    if (
      (!appSettings.agents || appSettings.agents.length === 0) &&
      !appSettings.onboardingDismissed
    ) {
      setShowOnboarding(true);
    }

    await projectStore.initFromSettings(
      appSettings.projectPaths ?? [],
      appSettings.projectPath ?? null,
      appSettings.archivedProjectPaths ?? [],
    );

    try {
      const allRepos = await ReposAPI.search("");
      sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
    } catch {}

    unlisteners.push(
      await onSessionCommunicationChanged(({ sessionId, communication }) => {
        sessionsStore.setCommunication(sessionId, communication);
      })
    );

    const sessions = await SessionAPI.list();
    sessionsStore.setSessions(sessions);

    const activeId = await SessionAPI.getActive();
    sessionsStore.setActiveId(activeId);

    window.addEventListener("focus", handleWindowFocusDriftRefresh);
    document.addEventListener("visibilitychange", handleWindowFocusDriftRefresh);

    unlisteners.push(
      await onSessionCreated((session) => {
        sessionsStore.addSession(session);
        if (
          sessionsStore.sessions.length === 1 &&
          !isExitedStatus(session.status)
        ) {
          sessionsStore.setActiveId(session.id);
        }
        scheduleProfileOutdatedRefresh();
      })
    );

    unlisteners.push(
      await onSessionDestroyed(({ id }) => {
        sessionsStore.removeSession(id);
        sessionsStore.setDetached(id, false);
      })
    );

    unlisteners.push(
      await onTerminalDetached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, true)
      )
    );

    unlisteners.push(
      await onTerminalAttached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, false)
      )
    );

    try {
      const ids = await WindowAPI.listDetached();
      ids.forEach((id) => sessionsStore.setDetached(id, true));
    } catch (e) {
      console.warn("[sidebar] listDetached hydration failed:", e);
    }

    unlisteners.push(
      await onSessionSwitched(({ id }) => {
        sessionsStore.setActiveId(id);
      })
    );

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        sessionsStore.renameSession(id, name);
      })
    );

    unlisteners.push(
      await onSessionIdle(({ id }) => {
        sessionsStore.markActivity(id);
        sessionsStore.setSessionWaiting(id, true);
      })
    );

    unlisteners.push(
      await onSessionBusy(({ id }) => {
        sessionsStore.setSessionWaiting(id, false);
      })
    );

    unlisteners.push(
      await onSessionContext(({ sessionId, percent }) => {
        sessionsStore.setSessionContext(sessionId, percent);
      })
    );

    try {
      await Promise.all(
        sessionsStore.sessions
          .filter((s) => s.agentId)
          .map(async (s) => {
            sessionsStore.hydrateSessionContext(s.id, await PtyAPI.getSessionContext(s.id));
          })
      );
    } catch {}

    unlisteners.push(
      await onSessionGitRepos(({ sessionId, repos }) => {
        sessionsStore.setGitRepos(sessionId, repos);
      })
    );

    unlisteners.push(
      await onWorkgroupTaskUpdated((data) => {
        const wgPath = data.workgroupRoot;
        if (wgPath) {
          projectStore.updateWorkgroupTask(wgPath, taskFirstLine(data.task), data.taskTitle);
        }
      })
    );

    unlisteners.push(
      await onSessionCoordinatorChanged(({ sessionId, isCoordinator }) => {
        sessionsStore.setIsCoordinator(sessionId, isCoordinator);
      })
    );

    const bridges = await TelegramAPI.listBridges();
    bridgesStore.setBridges(bridges);

    unlisteners.push(
      await onTelegramBridgeAttached((info) => {
        bridgesStore.addBridge(info);
      })
    );

    unlisteners.push(
      await onTelegramBridgeDetached(({ sessionId }) => {
        bridgesStore.removeBridge(sessionId);
      })
    );

    unlisteners.push(
      await onTelegramBridgeError(({ sessionId, error }) => {
        console.error(`Bridge error for ${sessionId}: ${error}`);
      })
    );

  });

  onCleanup(() => {
    disposed = true;
    unlisteners.forEach((unlisten) => unlisten());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
    if (stopTeamIdleWatcher) stopTeamIdleWatcher();
    if (loopToastTimer) clearTimeout(loopToastTimer);
    if (profileDriftRefreshTimer) clearTimeout(profileDriftRefreshTimer);
    sessionsStore.setSidebarPointerInside(false);
    document.removeEventListener("mousedown", handleRaiseTerminal);
    document.removeEventListener("contextmenu", blockContextMenu);
    window.removeEventListener("main-sidebar-side-change", handleMainSidebarSideChange);
    window.removeEventListener("focus", handleWindowFocusDriftRefresh);
    document.removeEventListener("visibilitychange", handleWindowFocusDriftRefresh);
  });

  return (
    <>
      <div
        class="sidebar-layout"
        onPointerEnter={() => sessionsStore.setSidebarPointerInside(true)}
        onPointerLeave={() => sessionsStore.setSidebarPointerInside(false)}
        data-ac-testid="sidebar.root"
        data-ac-role="surface"
      >
        <Show when={!props.embedded}>
          <Titlebar />
        </Show>
        <ActionBar />
        <RootAgentBanner />
        <div class="sidebar-body" data-rail-side={railSide()}>
          <Show when={railSide() === "left"}>
            <WorkgroupGroupRail projects={projectStore.projects} />
          </Show>
          <div class="sidebar-scrollable" ref={sidebarScrollableEl}>
            <ProjectPanel />
          </div>
          <Show when={railSide() === "right"}>
            <WorkgroupGroupRail projects={projectStore.projects} />
          </Show>
        </div>
      </div>
      <Show when={showOnboarding()}>
        <OnboardingModal onClose={() => setShowOnboarding(false)} />
      </Show>
      <Show when={loopToast()}>
        {(toast) => (
          <div class={toast().className} data-ac-testid="loop.toast">
            {toast().message}
          </div>
        )}
      </Show>
      <Show when={activeContextTemplateUpdate()}>
        {(update) => (
          <ContextTemplateUpdateModal
            update={update()}
            busy={contextTemplateUpdateBusy()}
            error={contextTemplateUpdateError()}
            onKeep={() => void keepContextTemplateUpdate()}
            onOverwrite={() => void overwriteContextTemplateUpdate()}
          />
        )}
      </Show>
      <AutoUnarchiveModal />
      <ToastHost />
    </>
  );
};

export default SidebarApp;
export { handleProjectRefreshRequested };
