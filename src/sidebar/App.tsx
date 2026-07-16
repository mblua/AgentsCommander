import { Component, createSignal, createEffect, createMemo, on, onMount, onCleanup, Show } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import type { TransportConnectionState } from "../shared/transport";
import type {
  SessionStatus,
  ContextTemplateUpdate,
  MainSidebarSide,
  SessionWarning,
  SessionSelection,
} from "../shared/types";
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
  getTransportConnectionState,
  isSelectionCoordinatorBusyError,
  onTransportConnectionState,
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
import { voiceRecorder } from "../shared/voice-recorder";
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
  /**
   * True when mounted inside MainApp's unified layout. Skips window-level
   * initializers; those are main-window concerns.
   */
  embedded?: boolean;
  railSide?: MainSidebarSide;
}

const HYDRATION_RETRY_DELAYS = [50, 100, 250, 500, 1000] as const;

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
  // Allow native menus where users expect text Copy/Paste.
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

/**
 * #941 — align the newly selected project's header with the top of the shared
 * sidebar scrollport. The primitive semantic key deliberately suppresses raw
 * store/config emissions and same-selection re-clicks.
 */
export function createSidebarSelectionScrollReset(
  scrollContainer: () => HTMLDivElement | undefined,
): void {
  const selectionKey = createMemo(activeWorkgroupGroupSelectionKey);
  let disposed = false;

  createEffect(on(selectionKey, (key) => {
    if (!key) return;
    const projectPath = workgroupGroupsStore.activeProjectPath();
    if (!projectPath) return;

    // Collapse/expand and the filtered rows update in the same click. Position
    // only after those Solid updates settle, and discard a superseded fast click.
    queueMicrotask(() => {
      if (disposed || selectionKey() !== key) return;
      const container = scrollContainer();
      if (!container) return;

      const projectPanel = projectPanelForPath(container, projectPath);
      if (projectPanel) {
        const containerTop = container.getBoundingClientRect().top;
        // The panel starts at its sticky header. Measure the non-sticky panel
        // because a pinned header's rect cannot reveal its logical scroll offset.
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
  const railSide = () => props.railSide ?? settingsRailSide();
  let sidebarScrollableEl: HTMLDivElement | undefined;
  createSidebarSelectionScrollReset(() => sidebarScrollableEl);
  // #592 - debounce handle for the profile-drift re-list (collapses bursts).
  let profileDriftRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  const handleMainSidebarSideChange = (event: Event) => {
    const side = (event as CustomEvent<{ side?: MainSidebarSide }>).detail?.side;
    setSettingsRailSide(side === "left" ? "left" : "right");
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

  const liveWarningsDuringInitialDrain = new Map<string, number>();
  let initialWarningDrainActive = false;
  let disposed = false;
  let hydrationRetryTimer: ReturnType<typeof setTimeout> | null = null;
  let hydrationRetryAttempt = 0;
  let hydrationInFlightGeneration: number | null = null;
  let observedConnection: TransportConnectionState = {
    state: "disconnected",
    generation: -1,
  };
  const mountDisposed = Symbol("sidebarAppMountDisposed");

  const addUnlistener = (unlisten: UnlistenFn): void => {
    if (disposed) {
      unlisten();
      return;
    }
    unlisteners.push(unlisten);
  };

  const register = async (registration: Promise<UnlistenFn>): Promise<void> => {
    const unlisten = await registration;
    if (disposed) {
      unlisten();
      throw mountDisposed;
    }
    unlisteners.push(unlisten);
  };

  const cancelHydrationRetry = (): void => {
    if (!hydrationRetryTimer) return;
    clearTimeout(hydrationRetryTimer);
    hydrationRetryTimer = null;
  };

  const applyAuthoritativeSelection = (
    selection: SessionSelection,
    generation: number,
    allowEqualReconnect: boolean,
  ): boolean => {
    if (disposed) return false;
    const accepted = sessionsStore.applySelection(
      selection,
      generation,
      allowEqualReconnect,
    );
    if (!accepted) return false;
    cancelHydrationRetry();
    hydrationRetryAttempt = 0;
    voiceRecorder.revokeLiveBinding();
    if (selection.mode === "live" && sessionsStore.activeId !== selection.id) {
      voiceRecorder.revokeSession(selection.id);
    }
    return true;
  };

  const scheduleHydrationRetry = (generation: number): void => {
    if (
      disposed ||
      hydrationRetryTimer ||
      observedConnection.state !== "connected" ||
      observedConnection.generation !== generation ||
      sessionsStore.awaitingHydrationGeneration !== generation
    ) {
      return;
    }
    const delay = HYDRATION_RETRY_DELAYS[
      Math.min(hydrationRetryAttempt, HYDRATION_RETRY_DELAYS.length - 1)
    ];
    hydrationRetryAttempt += 1;
    hydrationRetryTimer = setTimeout(() => {
      hydrationRetryTimer = null;
      void requestSelectionHydration(generation);
    }, delay);
  };

  const requestSelectionHydration = async (generation: number): Promise<void> => {
    if (
      disposed ||
      observedConnection.state !== "connected" ||
      observedConnection.generation !== generation ||
      hydrationInFlightGeneration === generation ||
      !sessionsStore.beginHydration(generation)
    ) {
      return;
    }
    hydrationInFlightGeneration = generation;
    try {
      const selection = await SessionAPI.getSelection();
      if (
        disposed ||
        observedConnection.state !== "connected" ||
        observedConnection.generation !== generation ||
        sessionsStore.awaitingHydrationGeneration !== generation
      ) {
        return;
      }
      applyAuthoritativeSelection(selection, generation, true);
    } catch (error) {
      if (
        disposed ||
        observedConnection.state !== "connected" ||
        observedConnection.generation !== generation ||
        sessionsStore.awaitingHydrationGeneration !== generation
      ) {
        return;
      }
      if (isSelectionCoordinatorBusyError(error)) {
        scheduleHydrationRetry(generation);
      } else {
        sessionsStore.cancelHydration(generation);
        console.error("[selection] Sidebar selection hydration failed:", error);
      }
    } finally {
      if (hydrationInFlightGeneration === generation) {
        hydrationInFlightGeneration = null;
      }
    }
  };

  const applyConnectionState = (connection: TransportConnectionState): void => {
    if (disposed) return;
    if (connection.generation < observedConnection.generation) return;
    if (
      connection.generation === observedConnection.generation &&
      connection.state === observedConnection.state
    ) {
      return;
    }
    const generationChanged = connection.generation !== observedConnection.generation;
    observedConnection = { ...connection };
    sessionsStore.observeConnection(connection);
    cancelHydrationRetry();
    hydrationRetryAttempt = 0;
    if (connection.state === "disconnected" || generationChanged) {
      voiceRecorder.revokeLiveBinding();
    }
    if (connection.state === "disconnected") {
      return;
    }
    void requestSelectionHydration(connection.generation);
  };

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
    try {
    // Selection authority and transport lifecycle are registered before every
    // hydration/list await so an event cannot lose a race to an older snapshot.
    await register(
      onSessionSwitched((selection, deliveryGeneration) => {
        applyAuthoritativeSelection(selection, deliveryGeneration, false);
      }),
    );
    if (disposed) return;
    await register(onTransportConnectionState(applyConnectionState));
    if (disposed) return;
    await register(
      onSessionCreated((session) => {
        sessionsStore.addSession(session);
        if (isExitedStatus(session.status)) voiceRecorder.revokeSession(session.id);
        scheduleProfileOutdatedRefresh();
      }),
    );
    if (disposed) return;
    await register(
      onSessionDestroyed(({ id }) => {
        voiceRecorder.revokeSession(id);
        sessionsStore.removeSession(id);
        sessionsStore.setDetached(id, false);
      }),
    );
    if (disposed) return;
    applyConnectionState(getTransportConnectionState());

    // #289 / dark-default — dark is the base CSS, so first paint is dark with
    // no optimistic class; the persisted-preference check after
    // SettingsAPI.get() below opts into light only for users who chose it last
    // session. Guarded with !props.embedded because MainApp owns the
    // documentElement classList when this is mounted inside the unified layout
    // — same pattern as zoom/geometry.
    // #912: subscribe before the startup warning drain. The backend appends
    // before live emit, so exact live/drained duplicates are collapsed below.
    await register(onSessionEnvWarning(handleLiveSessionWarning));
    void drainBufferedSessionWarnings();

    await register(
      onAcProjectRefreshRequested((data) => {
        handleProjectRefreshRequested(data);
      })
    );
    await register(
      onProjectGroupsUpdated((data) => {
        workgroupGroupsStore.applyExternalUpdate(data.projectPath, data.config);
      })
    );
    await register(
      onProjectArchiveChanged((data) => {
        void projectStore.applyArchiveChange(data).catch((error) => {
          console.error("[archive] Failed to apply project_archive_changed:", error);
        });
        if (data.reason === "autoUnarchive") {
          autoUnarchiveStore.push(data);
        }
      })
    );
    await register(
      onCodingAgentProfilesUpdated(() => {
        settingsStore.refresh();
        void refreshProfileOutdated();
      })
    );
    // #609 npm update notification. Subscribe BEFORE snapshotting so a startup
    // emit fired during mount is never dropped; showUpdateToast dedups on
    // version (subscribe-then-snapshot order). The snapshot
    // is fire-and-forget so its IPC round-trip never delays the listener
    // registrations that follow in this onMount.
    await register(
      onNpmUpdateAvailable((info) => showUpdateToast(info))
    );
    void SettingsAPI.getUpdateStatus()
      .then((pending) => {
        if (!disposed && pending) showUpdateToast(pending);
      })
      .catch((err) => {
        console.error("[update-check] getUpdateStatus failed:", err);
      });
    // #714 screenshot capture saved/failed toasts + startup hotkey-status warning.
    for (const unlisten of await wireScreenshotListeners()) addUnlistener(unlisten);
    if (disposed) return;
    await register(
      onCodingAgentEnvSettingsUpdated(() => {
        settingsStore.refresh();
        void refreshProfileOutdated();
      })
    );
    await register(
      onCodingAgentProfileSelectionUpdated((data) => {
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
    await register(
      onLoopEvent((data) => {
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
      if (disposed) {
        cleanupZoom();
        cleanupZoom = null;
        return;
      }
      cleanupGeometry = await initWindowGeometry("sidebar");
      if (disposed) {
        cleanupGeometry();
        cleanupGeometry = null;
        return;
      }
    }

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    if (disposed) return;
    setSettingsRailSide(appSettings.mainSidebarSide === "left" ? "left" : "right");
    if (!props.embedded) {
      document.documentElement.classList.toggle("light-theme", appSettings.themeLight);
    }
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    sessionsStore.setCoordSortByActivity(appSettings.coordSortByActivity ?? false);
    sessionsStore.setAlwaysShowSelectedWorkgroup(appSettings.alwaysShowSelectedWorkgroup ?? true);
    // #965 (RC-3) — restore the rail's collapse snapshot. MUST run before
    // projectStore.initFromSettings() populates the rail: that ordering is the only
    // thing preventing an expand-then-collapse flash, because onMount runs AFTER the
    // first paint. Deliberately NOT inside the `!props.embedded` guard above — the
    // rail is live in the browser client and in the unified main window too.
    railCollapseStore.hydrateFromSettings(appSettings);
    // Apply sidebar style from settings (remap removed themes to default)
    const style = appSettings.sidebarStyle;
    const removedThemes = ["classic", "signal-grid"];
    document.documentElement.dataset.sidebarStyle = (!style || removedThemes.includes(style)) ? "noir-minimal" : style;
    if (!props.embedded && appSettings.sidebarAlwaysOnTop && isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(true);
      if (disposed) return;
    }
    if (!props.embedded) {
      document.addEventListener("mousedown", handleRaiseTerminal);
    }

    // Block the default browser context menu globally — custom menus are used instead
    document.addEventListener("contextmenu", blockContextMenu);
    window.addEventListener("main-sidebar-side-change", handleMainSidebarSideChange);

    if (!props.embedded) {
      try {
        await applyWindowLayout("right");
      } catch {}
      if (disposed) return;
    }

    // Load settings into reactive store (for voice-to-text visibility etc.)
    await settingsStore.load();
    if (disposed) return;

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
      appSettings.archivedProjectPaths ?? [],
    );
    if (disposed) return;

    // Load all repos for inactive agent display
    try {
      const allRepos = await ReposAPI.search("");
      if (!disposed) sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
    } catch {}
    if (disposed) return;

    await register(
      onSessionCommunicationChanged(({ sessionId, communication }) => {
        sessionsStore.setCommunication(sessionId, communication);
      })
    );

    // Load initial sessions
    const sessions = await SessionAPI.list();
    if (disposed) return;
    sessionsStore.setSessions(sessions);

    // #592 - surface profile drift edited OUTSIDE the app (a hand edit to
    // settings.json, or any path that does not emit coding_agent_profiles_updated)
    // the moment the user returns to AC. The in-app edit events still refresh
    // immediately; this is the robust catch-all for everything else.
    window.addEventListener("focus", handleWindowFocusDriftRefresh);
    document.addEventListener("visibilitychange", handleWindowFocusDriftRefresh);

    // Listen for events
    await register(
      onTerminalDetached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, true)
      )
    );

    await register(
      onTerminalAttached(({ sessionId }) =>
        sessionsStore.setDetached(sessionId, false)
      )
    );

    // Hydrate detachedIds from backend (G.8 race safety — covers detach
    // events that fired before this component mounted, e.g. from the
    // Phase-3 restore path or from a prior detach survived across a
    // SidebarApp re-mount in the unified window).
    try {
      const ids = await WindowAPI.listDetached();
      if (!disposed) ids.forEach((id) => sessionsStore.setDetached(id, true));
    } catch (e) {
      console.warn("[sidebar] listDetached hydration failed:", e);
    }
    if (disposed) return;

    await register(
      onSessionRenamed(({ id, name }) => {
        sessionsStore.renameSession(id, name);
      })
    );

    await register(
      onSessionIdle(({ id }) => {
        sessionsStore.markActivity(id);
        sessionsStore.setSessionWaiting(id, true);
      })
    );

    await register(
      onSessionBusy(({ id }) => {
        sessionsStore.setSessionWaiting(id, false);
      })
    );

    await register(
      onSessionGitRepos(({ sessionId, repos }) => {
        sessionsStore.setGitRepos(sessionId, repos);
      })
    );

    await register(
      onWorkgroupTaskUpdated((data) => {
        const wgPath = data.workgroupRoot;
        if (wgPath) {
          projectStore.updateWorkgroupTask(wgPath, taskFirstLine(data.task), data.taskTitle);
        }
      })
    );

    await register(
      onSessionCoordinatorChanged(({ sessionId, isCoordinator }) => {
        sessionsStore.setIsCoordinator(sessionId, isCoordinator);
      })
    );

    // Load initial bridge state
    const bridges = await TelegramAPI.listBridges();
    if (disposed) return;
    bridgesStore.setBridges(bridges);

    // Telegram bridge events
    await register(
      onTelegramBridgeAttached((info) => {
        bridgesStore.addBridge(info);
      })
    );

    await register(
      onTelegramBridgeDetached(({ sessionId }) => {
        bridgesStore.removeBridge(sessionId);
      })
    );

    await register(
      onTelegramBridgeError(({ sessionId, error }) => {
        console.error(`Bridge error for ${sessionId}: ${error}`);
      })
    );
    } catch (error) {
      if (error !== mountDisposed) throw error;
    }
  });

  onCleanup(() => {
    disposed = true;
    cancelHydrationRetry();
    sessionsStore.cancelHydration();
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
