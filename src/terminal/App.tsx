import { Component, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import { isTauri } from "../shared/platform";
import {
  SessionAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
  onThemeChanged,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry } from "../shared/window-geometry";
import { settingsStore } from "../shared/stores/settings";
import { terminalStore } from "./stores/terminal";
import Titlebar from "./components/Titlebar";
import LastPrompt from "./components/LastPrompt";
import TerminalView from "./components/TerminalView";
import StatusBar from "./components/StatusBar";
import "./styles/terminal.css";

interface TerminalAppProps {
  lockedSessionId?: string;
  detached?: boolean;
}

const TerminalApp: Component<TerminalAppProps> = (props) => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;

  const loadActiveSession = async () => {
    if (props.lockedSessionId) {
      // Detached mode: lock to specific session
      const sessions = await SessionAPI.list();
      const session = sessions.find((s) => s.id === props.lockedSessionId);
      if (session) {
        terminalStore.setActiveSession(session.id, session.name, session.shell, session.effectiveShellArgs, session.workingDirectory);
      } else {
        // Session no longer exists, close this window
        terminalStore.setActiveSession(null, "", "", null, "");
      }
      return;
    }

    // Normal mode: follow active session
    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell, active.effectiveShellArgs, active.workingDirectory);
      }
    } else {
      terminalStore.setActiveSession(null, "", "", null, "");
    }
  };

  onMount(async () => {
    document.documentElement.classList.add("light-theme");
    shortcutHandler = registerShortcuts();
    cleanupZoom = await initZoom("terminal");
    cleanupGeometry = await initWindowGeometry("terminal");
    settingsStore.load();
    await loadActiveSession();

    if (!props.lockedSessionId) {
      // Normal mode: respond to session switches
      unlisteners.push(
        await onSessionSwitched(async ({ id }) => {
          if (!id) {
            terminalStore.setActiveSession(null, "", "", null, "");
            return;
          }
          const sessions = await SessionAPI.list();
          const session = sessions.find((s) => s.id === id);
          if (session) {
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.effectiveShellArgs,
              session.workingDirectory
            );
          }
        })
      );

      unlisteners.push(
        await onSessionCreated((session) => {
          if (!terminalStore.activeSessionId) {
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell,
              session.effectiveShellArgs,
              session.workingDirectory
            );
          }
        })
      );
    }

    unlisteners.push(
      await onSessionDestroyed(async ({ id }) => {
        if (props.lockedSessionId && id === props.lockedSessionId) {
          // Our locked session was destroyed, close this detached window
          if (isTauri) {
            const { getCurrentWindow } = await import("@tauri-apps/api/window");
            getCurrentWindow().close();
          }
          return;
        }
        if (!props.lockedSessionId) {
          await loadActiveSession();
        }
      })
    );

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        if (id === terminalStore.activeSessionId) {
          terminalStore.setActiveSession(id, name);
        }
      })
    );

    // Theme sync: follow sidebar theme toggle
    unlisteners.push(
      await onThemeChanged(({ light }) => {
        if (light) {
          document.documentElement.classList.add("light-theme");
        } else {
          document.documentElement.classList.remove("light-theme");
        }
      })
    );
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
  });

  return (
    <div class="terminal-layout">
      <Titlebar detached={props.detached} />
      <LastPrompt sessionId={props.lockedSessionId} />
      <Show
        when={terminalStore.activeSessionId}
        fallback={
          <div class="terminal-empty">
            <span>
              {props.detached
                ? "Session closed"
                : "No active session"}
            </span>
            <Show when={!props.detached}>
              <button
                class="terminal-empty-btn"
                onClick={() => SessionAPI.create()}
              >
                + New Session
              </button>
            </Show>
          </div>
        }
      >
        <TerminalView />
      </Show>
      <StatusBar detached={props.detached} />
    </div>
  );
};

export default TerminalApp;
