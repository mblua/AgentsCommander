import { Component, onMount, onCleanup, createMemo, Show } from "solid-js";
import type { UnlistenFn } from "../shared/transport";
import { isTauri } from "../shared/platform";
import {
  SessionAPI,
  WindowAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
  onThemeChanged,
  onWorkgroupTaskUpdated,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { initZoom } from "../shared/zoom";
import { initWindowGeometry, initDetachedWindowGeometry } from "../shared/window-geometry";
import { settingsStore } from "../shared/stores/settings";
import { terminalStore } from "./stores/terminal";
import { homeStore } from "../main/stores/home";
import Titlebar from "./components/Titlebar";
import WorkgroupTask from "./components/WorkgroupTask";
import LastPrompt from "./components/LastPrompt";
import TerminalView from "./components/TerminalView";
import StatusBar from "./components/StatusBar";
import HomeView from "../main/components/HomeView";
import ExternalLinkConfirm from "../shared/components/ExternalLinkConfirm";
import "../shared/styles/external-link-confirm.css";
import "./styles/terminal.css";

interface TerminalAppProps {
  lockedSessionId?: string;
  detached?: boolean;
  embedded?: boolean;
}

const TerminalApp: Component<TerminalAppProps> = (props) => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;

  const isHomeShown = createMemo(
    () => !!(props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible)
  );

  const loadActiveSession = async () => {
    if (props.lockedSessionId) {
      const sessions = await SessionAPI.list();
      const session = sessions.find((s) => s.id === props.lockedSessionId);
      if (session) {
        terminalStore.setActiveSession(session.id, session.name, session.shell, session.effectiveShellArgs, session.workingDirectory, session.workgroupTask ?? null, session.isRootAgent);
      } else {
        terminalStore.setActiveSession(null, "", "", null, "", null, false);
      }
      return;
    }

    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell, active.effectiveShellArgs, active.workingDirectory, active.workgroupTask ?? null, active.isRootAgent);
      }
    } else {
      terminalStore.setActiveSession(null, "", "", null, "", null, false);
    }
  };

  onMount(async () => {
    shortcutHandler = registerShortcuts();

    unlisteners.push(
      await onSessionDestroyed(async ({ id }) => {
        if (props.lockedSessionId && id === props.lockedSessionId) {
          if (isTauri) {
            const { getCurrentWindow } = await import("@tauri-apps/api/window");
            getCurrentWindow().destroy();
          }
          return;
        }
        if (!props.lockedSessionId) {
          await loadActiveSession();
        }
      })
    );

    if (isTauri && props.detached && props.lockedSessionId) {
      const sessionId = props.lockedSessionId;
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const unlistenCloseRequested = await win.onCloseRequested(async (e) => {
        e.preventDefault();
        try {
          await WindowAPI.attach(sessionId);
        } catch (err) {
          console.error("[detached] attach failed during close; destroying window:", err);
          try { await win.destroy(); } catch { /* best-effort */ }
        }
      });
      unlisteners.push(unlistenCloseRequested);
    }

    if (!props.embedded) {
      cleanupZoom = await initZoom(props.detached ? "detached" : "terminal");
      if (props.detached && props.lockedSessionId) {
        cleanupGeometry = await initDetachedWindowGeometry(props.lockedSessionId);
      } else {
        cleanupGeometry = await initWindowGeometry("terminal");
      }
    }
    await settingsStore.load();
    if (!props.embedded) {
      document.documentElement.classList.toggle("light-theme", !!settingsStore.current?.themeLight);
    }
    await loadActiveSession();

    if (!props.lockedSessionId) {
      unlisteners.push(
        await onSessionSwitched(async ({ id }) => {
          if (!id) {
            terminalStore.setActiveSession(null, "", "", null, "", null, false);
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
              session.workingDirectory,
              session.workgroupTask ?? null,
              session.isRootAgent
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
              session.workingDirectory,
              session.workgroupTask ?? null,
              session.isRootAgent
            );
          }
        })
      );
    }

    unlisteners.push(
      await onSessionRenamed(({ id, name }) => {
        if (id === terminalStore.activeSessionId) {
          terminalStore.setActiveSession(id, name);
        }
      })
    );


    const normalizePathForCompare = (p: string): string => {
      let s = p;
      if (s.startsWith("\\\\?\\")) s = s.slice(4);
      else if (s.startsWith("//?/")) s = s.slice(4);
      return s.replace(/\\/g, "/").toLowerCase();
    };
    unlisteners.push(
      await onWorkgroupTaskUpdated((data) => {
        if (data.source === "poll") {
          const targetId = props.lockedSessionId ?? terminalStore.activeSessionId;
          if (!targetId) return;
          if (!data.sessionIds.includes(targetId)) return;
          terminalStore.setActiveWorkgroupTask(data.task);
        }
        else if (data.source === "manual") {
          const wgRoot = data.workgroupRoot;
          const cwd = terminalStore.activeWorkingDirectory;
          if (!cwd || !wgRoot) return;
          const cwdNorm = normalizePathForCompare(cwd);
          const wgNorm = normalizePathForCompare(wgRoot);
          if (cwdNorm === wgNorm || cwdNorm.startsWith(wgNorm + "/")) {
            terminalStore.setActiveWorkgroupTask(data.task);
          }
        }
      })
    );

    if (!props.embedded) {
      unlisteners.push(
        await onThemeChanged(({ light }) => {
          if (light) {
            document.documentElement.classList.add("light-theme");
          } else {
            document.documentElement.classList.remove("light-theme");
          }
        })
      );
    }
  });

  onCleanup(() => {
    unlisteners.forEach((u) => u());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    if (cleanupZoom) cleanupZoom();
    if (cleanupGeometry) cleanupGeometry();
  });

  return (
    <div
      class="terminal-layout"
      data-ac-testid="terminal.root"
      data-ac-role="surface"
      data-ac-state={terminalStore.activeSessionId ? "active" : "empty"}
    >
      <Show when={!props.embedded}>
        <Titlebar detached={props.detached} lockedSessionId={props.lockedSessionId} />
      </Show>
      {/* #771 — the TASK panel is hidden entirely for the Root Agent
          (Agent's Commander); LAST PROMPT stays for every agent. Gated on the
          terminalStore flag (not sessionsStore) so it works in the standalone
          and detached terminal windows too, which don't load sidebar state. */}
      <Show when={!terminalStore.activeIsRootAgent}>
        <WorkgroupTask />
      </Show>
      <LastPrompt sessionId={props.lockedSessionId} />
      <div class="terminal-content-area">
        <Show
          when={terminalStore.activeSessionId}
          fallback={
            <div
              class="terminal-empty"
              data-ac-testid="terminal.empty"
              data-ac-role="status"
            >
              <span>
                {props.detached
                  ? "Session closed"
                  : "No active session"}
              </span>
            </div>
          }
        >
          <TerminalView lockedSessionId={props.lockedSessionId} />
        </Show>
      </div>
      <StatusBar detached={props.detached} />
      {/* Home overlay (issue #164). Sibling of WorkgroupTask/LastPrompt and
          .terminal-content-area inside .terminal-layout (the positioned
          containing block). Painted on top so it visually covers TASK /
          LAST PROMPT and the terminal area, but those panels remain mounted
          underneath — toggling Home must not change the height of
          .terminal-content-area or trigger TerminalView's ResizeObserver
          (which would SIGWINCH the PTY). TerminalView is never unmounted
          while Home is visible. Detached/locked windows never render Home. */}
      <Show when={isHomeShown()}>
        <HomeView />
      </Show>
      <Show when={isTauri && !props.embedded}>
        <ExternalLinkConfirm />
      </Show>
    </div>
  );
};

export default TerminalApp;
