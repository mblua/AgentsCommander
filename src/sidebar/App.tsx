import { Component, createSignal, createEffect, onMount, onCleanup, Show } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import type { SessionStatus, ContextTemplateUpdate } from "../shared/types";
import {
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
  onSessionGitRepos,
  onSessionCoordinatorChanged,
  onTelegramBridgeAttached,
  onTelegramBridgeDetached,
  onTelegramBridgeError,
  onTerminalDetached,
  onTerminalAttached,
  onWorkgroupTaskUpdated,
  onAcProjectRefreshRequested,
  onCodingAgentProfilesUpdated,
  onCodingAgentEnvSettingsUpdated,
  onCodingAgentProfileSelectionUpdated,
  onLoopEvent,
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
import { startTeamIdleWatcher } from "./stores/team-idle-watcher";
import { primeAudio } from "../shared/sound";
import { settingsStore } from "../shared/stores/settings";
import Titlebar from "./components/Titlebar";
import ActionBar from "./components/ActionBar";
import RootAgentBanner from "./components/RootAgentBanner";
import ProjectPanel from "./components/ProjectPanel";
import OnboardingModal from "./components/OnboardingModal";
import ContextTemplateUpdateModal from "./components/ContextTemplateUpdateModal";
import ToastHost from "../shared/components/ToastHost";
import { toastStore } from "../shared/stores/toasts";
import { handleProjectRefreshRequested } from "./project-refresh-handler";
import { loopToastFromEvent, type LoopToast } from "./loop-event-toast";
import { createUpdateToaster } from "./update-toast";
import "./styles/sidebar.css";
import "../shared/styles/toast.css";

interface SidebarAppProps {
  /**
   * True when mounted inside MainApp's unified layout. Skips window-level
   * initializers; those are main-window concerns.
   */
  embedded?: boolean;
}

function isExitedStatus(status: SessionStatus): boolean {
  return typeof status === "object" && status !== null && "exited" in status;
}

const SidebarApp: Component<SidebarAppProps> = (props) => {
  const [showOnboarding, setShowOnboarding] = createSignal(false);
  const [loopToast, setLoopToast] = createSignal<LoopToast | null>(null);
  // #695 — one-at-a-time seeded context-template update modal. `seen` is keyed
  // by `(projectPath, filename, defaultSha256, fileSha256)` so a resolved or
  // skipped update is not re-shown, while a genuinely new pending update (the
  // user edited the file again, or the baked default bumped) gets a fresh key.
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
  // #592 - debounce handle for the profile-drift re-list (collapses bursts).
  let profileDriftRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  const blockContextMenu = (e: Event) => {
    // Allow the WebView2 native menu over the embedded terminal so users get
    // Copy/Paste. Custom menus elsewhere (SessionItem, ProjectPanel, etc.)
    // remain blocked.
    if (e.target instanceof Element && e.target.closest(".terminal-host")) return;
    e.preventDefault();
  };

  const handleRaiseTerminal = async (e: MouseEvent) => {
    if (!isTauri || props.embedded || !raiseTerminalEnabled) return;
    // Don't steal focus from interactive elements
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

  // #609 — sticky "npm update available" toaster. Per-mount dedup state so the
  // startup event + the getUpdateStatus snapshot never double-toast a version.
  const showUpdateToast = createUpdateToaster();

  // #695 — seeded context-template update flow. The exact removal key (grinch
  // fix #5) includes the file hash so a resolved older modal never silently
  // drops a newer pending update for the same project/file/default.
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

  // Drop exactly the resolved update from the store and close the modal. The
  // createEffect below then surfaces the next unseen pending update, one at a
  // time. Removal and `seen` both key on the file hash, so a concurrent newer
  // pending update survives (grinch fix #5).
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
      // Keep the modal open so the user can retry; the backend made no durable
      // change, so re-showing the same notice later would be correct anyway.
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
      // Sticky info toast — the user must be able to find the `.bak` the old
      // file was preserved as.
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

  // Surface the next pending context-template update whenever no modal is open.
  // Short-circuiting on an active modal keeps it one-at-a-time; closing the
  // modal re-runs this effect, which re-reads projectStore.projects and picks
  // up any update queued while the previous one was open.
  createEffect(() => {
    if (activeContextTemplateUpdate()) return;
    const next = nextContextTemplateUpdate();
    if (!next) return;
    seenContextTemplateUpdates.add(contextTemplateUpdateKey(next));
    setContextTemplateUpdateError(null);
    setActiveContextTemplateUpdate(next);
  });

  // #592 - pull the backend-computed drift flag and surgically patch each
  // session. Uses setProfileOutdated (never setSessions) so the frontend-only
  // pendingReview field is preserved. list_sessions recomputes profileOutdated
  // from the persisted replica hash vs the current config, so this surfaces drift
  // from ANY source: a profile event, an external settings.json edit, or
  // pre-existing drift restored at boot.
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
  // Debounce so a burst (a workgroup spawn emitting many session_created, or a
  // broad-scope profile apply touching many replicas) collapses into a single
  // list_sessions round-trip.
  const scheduleProfileOutdatedRefresh = () => {
    if (profileDriftRefreshTimer) clearTimeout(profileDriftRefreshTimer);
    profileDriftRefreshTimer = setTimeout(() => {
      profileDriftRefreshTimer = null;
      void refreshProfileOutdated();
    }, 250);
  };
  // Re-check drift when the window regains focus / becomes visible. The robust
  // catch-all for a cell edited OUTSIDE the app (a hand edit to settings.json,
  // or any path that does not emit coding_agent_profiles_updated): the badge
  // lights up as soon as the user returns to AC.
  const handleWindowFocusDriftRefresh = () => {
    if (document.visibilityState === "hidden") return;
    scheduleProfileOutdatedRefresh();
  };

  onMount(async () => {
    // #289 / dark-default — dark is the base CSS, so first paint is dark with
    // no optimistic class; the persisted-preference check after
    // SettingsAPI.get() below opts into light only for users who chose it last
    // session. Guarded with !props.embedded because MainApp owns the
    // documentElement classList when this is mounted inside the unified layout
    // — same pattern as zoom/geometry.
    unlisteners.push(
      await onAcProjectRefreshRequested((data) => {
        handleProjectRefreshRequested(data);
      })
    );
    unlisteners.push(
      await onCodingAgentProfilesUpdated(() => {
        settingsStore.refresh();
        void refreshProfileOutdated();
      })
    );
    // #609 npm update notification. Subscribe BEFORE snapshotting so a startup
    // emit fired during mount is never dropped; showUpdateToast dedups on
    // version (mirrors RtkBanner's subscribe-then-snapshot order). The snapshot
    // is fire-and-forget so its IPC round-trip never delays the listener
    // registrations that follow in this onMount.
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
          // Broad-scope apply (#384) touched many replicas with no single
          // agentPath — reload every loaded project so discovery refreshes
          // currentCodingAgentId/currentProfile everywhere.
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

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    if (!props.embedded) {
      document.documentElement.classList.toggle("light-theme", appSettings.themeLight);
    }
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    sessionsStore.setCoordSortByActivity(appSettings.coordSortByActivity ?? false);
    sessionsStore.setAlwaysShowSelectedWorkgroup(appSettings.alwaysShowSelectedWorkgroup ?? true);
    // Apply sidebar style from settings (remap removed themes to default)
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

    // Block the default browser context menu globally — custom menus are used instead
    document.addEventListener("contextmenu", blockContextMenu);

    if (!props.embedded) {
      try {
        await applyWindowLayout("right");
      } catch {}
    }

    // Load settings into reactive store (for voice-to-text visibility etc.)
    await settingsStore.load();

    // Feature #110 — start watching for team busy→all-idle transitions.
    // Mounted after settingsStore.load() so the very first effect run
    // already sees the user's teamIdleBeepEnabled choice; the watcher's
    // own initialization guard separately suppresses any startup beep.
    // primeAudio() arms the AudioContext on first user gesture so the
    // very first beep isn't swallowed by browser autoplay policy.
    primeAudio();
    stopTeamIdleWatcher = startTeamIdleWatcher();

    // First-run: show onboarding if no coding agents configured and not previously dismissed
    if (
      (!appSettings.agents || appSettings.agents.length === 0) &&
      !appSettings.onboardingDismissed
    ) {
      setShowOnboarding(true);
    }

    // Load saved project if any
    await projectStore.initFromSettings(
      appSettings.projectPaths ?? [],
      appSettings.projectPath ?? null,
    );

    // Load all repos for inactive agent display
    try {
      const allRepos = await ReposAPI.search("");
      sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
    } catch {}

    unlisteners.push(
      await onSessionCommunicationChanged(({ sessionId, communication }) => {
        sessionsStore.setCommunication(sessionId, communication);
      })
    );

    // Load initial sessions
    const sessions = await SessionAPI.list();
    sessionsStore.setSessions(sessions);

    const activeId = await SessionAPI.getActive();
    sessionsStore.setActiveId(activeId);

    // #592 - surface profile drift edited OUTSIDE the app (a hand edit to
    // settings.json, or any path that does not emit coding_agent_profiles_updated)
    // the moment the user returns to AC. The in-app edit events still refresh
    // immediately; this is the robust catch-all for everything else.
    window.addEventListener("focus", handleWindowFocusDriftRefresh);
    document.addEventListener("visibilitychange", handleWindowFocusDriftRefresh);

    // Listen for events
    unlisteners.push(
      await onSessionCreated((session) => {
        sessionsStore.addSession(session);
        // New session is auto-activated if it's the first one
        if (
          sessionsStore.sessions.length === 1 &&
          !isExitedStatus(session.status)
        ) {
          sessionsStore.setActiveId(session.id);
        }
        // #592 - session_created carries profileOutdated=false (From<&Session>
        // cannot compute drift). A session restored at boot with pre-existing
        // drift must re-derive it from list_sessions, which would otherwise be
        // clobbered to false by the addSession upsert. Debounced so a workgroup
        // spawn burst collapses into one re-list.
        scheduleProfileOutdatedRefresh();
      })
    );

    unlisteners.push(
      await onSessionDestroyed(({ id }) => {
        sessionsStore.removeSession(id);
        // Detached-window cleanup: if the session had a detached window,
        // its destroy also closes that window. Clear the store flag so
        // UI (icons, menu items) doesn't linger in detached state.
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

    // Hydrate detachedIds from backend (G.8 race safety — covers detach
    // events that fired before this component mounted, e.g. from the
    // Phase-3 restore path or from a prior detach survived across a
    // SidebarApp re-mount in the unified window).
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

    // Load initial bridge state
    const bridges = await TelegramAPI.listBridges();
    bridgesStore.setBridges(bridges);

    // Telegram bridge events
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
        <div class="sidebar-scrollable">
          <ProjectPanel />
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
      <ToastHost />
    </>
  );
};

export default SidebarApp;
export { handleProjectRefreshRequested };
