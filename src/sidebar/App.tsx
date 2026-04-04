import { Component, onMount, onCleanup } from "solid-js";
import { isTauri } from "../shared/platform";
import type { UnlistenFn } from "../shared/transport";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  DarkFactoryAPI,
  ReposAPI,
  WindowAPI,
  onSessionCreated,
  onSessionDestroyed,
  onSessionSwitched,
  onSessionRenamed,
  onSessionIdle,
  onSessionBusy,
  onSessionGitBranch,
  onTelegramBridgeAttached,
  onTelegramBridgeDetached,
  onTelegramBridgeError,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { applyWindowLayout } from "../shared/window-layout";
import { sessionsStore } from "./stores/sessions";
import { bridgesStore } from "./stores/bridges";
import { projectStore } from "./stores/project";
import { settingsStore } from "../shared/stores/settings";
import Titlebar from "./components/Titlebar";
import ActionBar from "./components/ActionBar";
import ProjectPanel from "./components/ProjectPanel";
import "./styles/sidebar.css";

const SidebarApp: Component = () => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let raiseTerminalEnabled = true;
  let lastRaiseTime = 0;

  const handleRaiseTerminal = async (e: MouseEvent) => {
    if (!isTauri || !raiseTerminalEnabled) return;
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
    document.documentElement.classList.add("light-theme");
    shortcutHandler = registerShortcuts();
    cleanupZoom = await initZoom("sidebar");
    cleanupGeometry = await initWindowGeometry("sidebar");

    // Apply window settings
    const appSettings = await SettingsAPI.get();
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    // Apply sidebar style from settings
    if (appSettings.sidebarStyle && appSettings.sidebarStyle !== "classic") {
      document.documentElement.dataset.sidebarStyle = appSettings.sidebarStyle;
    }
    if (appSettings.sidebarAlwaysOnTop && isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(true);
    }
    document.addEventListener("mousedown", handleRaiseTerminal);

    // Default layout: sidebar right when no saved geometry
    if (!appSettings.sidebarGeometry && !appSettings.terminalGeometry) {
      try {
        await applyWindowLayout("right");
      } catch {}
    }

    // Load settings into reactive store (for voice-to-text visibility etc.)
    await settingsStore.load();

    // Load saved project if any
    await projectStore.initFromSettings(appSettings.projectPath ?? null);

    // Load all repos for inactive agent display
    try {
      const allRepos = await ReposAPI.search("");
      sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
    } catch {}

    // Load teams for filter
    try {
      const dfConfig = await DarkFactoryAPI.get();
      sessionsStore.setTeams(dfConfig.teams);
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
        if (sessionsStore.sessions.length === 1) {
          sessionsStore.setActiveId(session.id);
        }
      })
    );

    unlisteners.push(
      await onSessionDestroyed(({ id }) => {
        sessionsStore.removeSession(id);
      })
    );

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
        sessionsStore.setSessionWaiting(id, true);
      })
    );

    unlisteners.push(
      await onSessionBusy(({ id }) => {
        sessionsStore.setSessionWaiting(id, false);
      })
    );

    unlisteners.push(
      await onSessionGitBranch(({ sessionId, branch }) => {
        sessionsStore.setGitBranch(sessionId, branch);
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
    document.removeEventListener("mousedown", handleRaiseTerminal);
  });

  return (
    <div class="sidebar-layout">
      <Titlebar />
      <ActionBar />
      <div class="sidebar-scrollable">
        <ProjectPanel />
      </div>
    </div>
  );
};

export default SidebarApp;
