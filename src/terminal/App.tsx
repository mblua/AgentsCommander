import { Component, onMount, onCleanup, Show } from "solid-js";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  SessionAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { terminalStore } from "./stores/terminal";
import Titlebar from "./components/Titlebar";
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

  const loadActiveSession = async () => {
    if (props.lockedSessionId) {
      // Detached mode: lock to specific session
      const sessions = await SessionAPI.list();
      const session = sessions.find((s) => s.id === props.lockedSessionId);
      if (session) {
        terminalStore.setActiveSession(session.id, session.name, session.shell);
      } else {
        // Session no longer exists, close this window
        terminalStore.setActiveSession(null, "", "");
      }
      return;
    }

    // Normal mode: follow active session
    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell);
      }
    } else {
      terminalStore.setActiveSession(null, "", "");
    }
  };

  onMount(async () => {
    shortcutHandler = registerShortcuts();
    await loadActiveSession();

    if (!props.lockedSessionId) {
      // Normal mode: respond to session switches
      unlisteners.push(
        await onSessionSwitched(async ({ id }) => {
          if (!id) {
            terminalStore.setActiveSession(null, "", "");
            return;
          }
          const sessions = await SessionAPI.list();
          const session = sessions.find((s) => s.id === id);
          if (session) {
            terminalStore.setActiveSession(
              session.id,
              session.name,
              session.shell
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
              session.shell
            );
          }
        })
      );
    }

    unlisteners.push(
      await onSessionDestroyed(async ({ id }) => {
        if (props.lockedSessionId && id === props.lockedSessionId) {
          // Our locked session was destroyed, close this detached window
          const appWindow = getCurrentWindow();
          appWindow.close();
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
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
  });

  return (
    <div class="terminal-layout">
      <Titlebar detached={props.detached} />
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
