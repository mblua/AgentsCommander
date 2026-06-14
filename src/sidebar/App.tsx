import { Component, createSignal, onMount, onCleanup, Show } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import type { SessionStatus } from "../shared/types";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  ReposAPI,
  WindowAPI,
  onSessionCreated,
  onSessionDestroyed,
  onSessionSwitched,
  onSessionRenamed,
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
import { handleProjectRefreshRequested } from "./project-refresh-handler";
import "./styles/sidebar.css";

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
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let stopTeamIdleWatcher: (() => void) | null = null;
  let raiseTerminalEnabled = true;
  let lastRaiseTime = 0;
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

  onMount(async () => {
    // #289 — optimistically paint in light mode (the historic default and the
    // AppSettings default) to keep first paint flash-free for the common case.
    // The persisted-preference check after SettingsAPI.get() below overrides
    // back to dark for users who chose it last session. Guarded with
    // !props.embedded because MainApp owns the documentElement classList when
    // this is mounted inside the unified layout — same pattern as zoom/geometry.
    if (!props.embedded) {
      document.documentElement.classList.add("light-theme");
    }
    unlisteners.push(
      await onAcProjectRefreshRequested((data) => {
        handleProjectRefreshRequested(data);
      })
    );
    unlisteners.push(
      await onCodingAgentProfilesUpdated(() => {
        settingsStore.refresh();
      })
    );
    unlisteners.push(
      await onCodingAgentEnvSettingsUpdated(() => {
        settingsStore.refresh();
      })
    );
    unlisteners.push(
      await onCodingAgentProfileSelectionUpdated((data) => {
        settingsStore.refresh();
        void projectStore.reloadProjectIfLoaded(data.agentPath);
      })
    );

    shortcutHandler = registerShortcuts();
    if (!props.embedded) {
      cleanupZoom = await initZoom("sidebar");
      cleanupGeometry = await initWindowGeometry("sidebar");
    }

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    if (!props.embedded && !appSettings.themeLight) {
      document.documentElement.classList.remove("light-theme");
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

    // Load initial sessions
    const sessions = await SessionAPI.list();
    sessionsStore.setSessions(sessions);

    const activeId = await SessionAPI.getActive();
    sessionsStore.setActiveId(activeId);

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
    sessionsStore.setSidebarPointerInside(false);
    document.removeEventListener("mousedown", handleRaiseTerminal);
    document.removeEventListener("contextmenu", blockContextMenu);
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
    </>
  );
};

export default SidebarApp;
export { handleProjectRefreshRequested };
