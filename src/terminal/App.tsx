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
import "./styles/terminal.css";

interface TerminalAppProps {
  lockedSessionId?: string;
  detached?: boolean;
  /**
   * True when mounted inside MainApp's unified layout. Skips titlebar
   * render, window-level initializers, and redundant theme listener
   * (DW.2 + DW.5 + Arb-4).
   */
  embedded?: boolean;
}

const TerminalApp: Component<TerminalAppProps> = (props) => {
  const unlisteners: UnlistenFn[] = [];
  let shortcutHandler: ((e: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;

  // Home is rendered as an overlay covering the BRIEF/LAST PROMPT panels and
  // the terminal content area beneath them, while those panels stay mounted
  // (#164 follow-up). Keeping Brief/LastPrompt mounted preserves the height
  // of `.terminal-content-area`, so TerminalView's ResizeObserver does not
  // fire on Home toggle and the PTY does not receive a SIGWINCH. Detached
  // and locked windows never render Home — they keep the normal layout.
  const isHomeShown = createMemo(
    () => !!(props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible)
  );

  const loadActiveSession = async () => {
    if (props.lockedSessionId) {
      // Detached mode: lock to specific session
      const sessions = await SessionAPI.list();
      const session = sessions.find((s) => s.id === props.lockedSessionId);
      if (session) {
        terminalStore.setActiveSession(session.id, session.name, session.shell, session.effectiveShellArgs, session.workingDirectory, session.workgroupTask ?? null);
      } else {
        // Session no longer exists, close this window
        terminalStore.setActiveSession(null, "", "", null, "", null);
      }
      return;
    }

    // Normal mode: follow active session
    const activeId = await SessionAPI.getActive();
    if (activeId) {
      const sessions = await SessionAPI.list();
      const active = sessions.find((s) => s.id === activeId);
      if (active) {
        terminalStore.setActiveSession(active.id, active.name, active.shell, active.effectiveShellArgs, active.workingDirectory, active.workgroupTask ?? null);
      }
    } else {
      terminalStore.setActiveSession(null, "", "", null, "", null);
    }
  };

  onMount(async () => {
    // #289 — optimistic light-mode first paint (legacy default); the
    // settingsStore.load() corrective step below restores dark for users
    // who persisted that preference. Guarded with !props.embedded because
    // MainApp owns the documentElement classList when this is mounted inside
    // the unified layout — same pattern as zoom/geometry/onThemeChanged below.
    if (!props.embedded) {
      document.documentElement.classList.add("light-theme");
    }
    shortcutHandler = registerShortcuts();

    // Register destroy listener FIRST to catch any destroy event fired
    // during the async awaits below (A2.3.G7 mount-race window).
    unlisteners.push(
      await onSessionDestroyed(async ({ id }) => {
        if (props.lockedSessionId && id === props.lockedSessionId) {
          // Our locked session was destroyed, close this detached window.
          // R.2 discipline: destroy() not close() so onCloseRequested is
          // not fired (avoids looping into attach_terminal on a dead session).
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

    // Detached-window X → re-attach to main, not destroy (plan §A2.2.G4 / G.13).
    // Register as early as possible in onMount so the race window from first
    // paint to handler-registered is minimized. If attach fails (session
    // destroyed mid-flight, backend command error, etc.), fall back to
    // destroying the window so the user isn't stuck.
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

    // Window-level initializers — skipped when embedded (main owns these).
    if (!props.embedded) {
      // Detached windows use "detached" (mapped to terminalZoom in zoomKeyMap).
      cleanupZoom = await initZoom(props.detached ? "detached" : "terminal");
      if (props.detached && props.lockedSessionId) {
        // Per-session geometry persistence (plan §A2.4.Arb1).
        cleanupGeometry = await initDetachedWindowGeometry(props.lockedSessionId);
      } else {
        cleanupGeometry = await initWindowGeometry("terminal");
      }
    }
    await settingsStore.load();
    // #289 — restore persisted theme. Awaited above (vs. fire-and-forget)
    // so themeLight is known before the corrective remove runs. Embedded
    // children skip per the convention above.
    if (!props.embedded && !settingsStore.current?.themeLight) {
      document.documentElement.classList.remove("light-theme");
    }
    await loadActiveSession();

    if (!props.lockedSessionId) {
      // Normal mode: respond to session switches
      unlisteners.push(
        await onSessionSwitched(async ({ id }) => {
          if (!id) {
            terminalStore.setActiveSession(null, "", "", null, "", null);
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
              session.workgroupTask ?? null
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
              session.workgroupTask ?? null
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


    // Issue #162 & #163 merged: cross-window brief refresh and polling updates.
    const normalizePathForCompare = (p: string): string => {
      let s = p;
      if (s.startsWith("\\\\?\\")) s = s.slice(4);
      else if (s.startsWith("//?/")) s = s.slice(4);
      return s.replace(/\\/g, "/").toLowerCase();
    };
    unlisteners.push(
      await onWorkgroupTaskUpdated((data) => {
        // #163: Poller (ac_discovery.rs) provides sessionIds
        if (data.sessionIds) {
          const targetId = props.lockedSessionId ?? terminalStore.activeSessionId;       
          if (!targetId) return;
          if (!data.sessionIds.includes(targetId)) return;
          terminalStore.setActiveWorkgroupTask(data.task);
        } 
        // #162: Manual edits (brief.rs) provide workgroupRoot
        else if (data.workgroupRoot || data.workgroupPath) {
          const wgRoot = data.workgroupRoot || data.workgroupPath;
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

    // Theme sync: follow sidebar theme toggle (redundant in embedded mode —
    // sidebar's toggle already flips the shared documentElement classList).
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
    <div class="terminal-layout">
      <Show when={!props.embedded}>
        <Titlebar detached={props.detached} lockedSessionId={props.lockedSessionId} />
      </Show>
      <WorkgroupTask />
      <LastPrompt sessionId={props.lockedSessionId} />
      <div class="terminal-content-area">
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
                  onClick={() => { homeStore.hide(); SessionAPI.create(); }}
                >
                  + New Session
                </button>
              </Show>
            </div>
          }
        >
          <TerminalView lockedSessionId={props.lockedSessionId} />
        </Show>
      </div>
      <StatusBar detached={props.detached} />
      {/* Home overlay (issue #164). Sibling of WorkgroupTask/LastPrompt and
          .terminal-content-area inside .terminal-layout (the positioned
          containing block). Painted on top so it visually covers BRIEF /
          LAST PROMPT and the terminal area, but those panels remain mounted
          underneath — toggling Home must not change the height of
          .terminal-content-area or trigger TerminalView's ResizeObserver
          (which would SIGWINCH the PTY). TerminalView is never unmounted
          while Home is visible. Detached/locked windows never render Home. */}
      <Show when={isHomeShown()}>
        <HomeView />
      </Show>
    </div>
  );
};

export default TerminalApp;
