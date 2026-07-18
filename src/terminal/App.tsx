import { Component, createMemo, onCleanup, onMount, Show } from "solid-js";
import type { SessionSelection } from "../shared/types";
import type { TransportConnectionState, UnlistenFn } from "../shared/transport";
import { isTauri } from "../shared/platform";
import {
  getTransportConnectionState,
  isSelectionCoordinatorBusyError,
  onSessionDestroyed,
  onSessionRenamed,
  onSessionSwitched,
  onThemeChanged,
  onTransportConnectionState,
  onWorkgroupTaskUpdated,
  SessionAPI,
  WindowAPI,
} from "../shared/ipc";
import { registerShortcuts, unregisterShortcuts } from "../shared/shortcuts";
import { voiceRecorder } from "../shared/voice-recorder";
import { initZoom } from "../shared/zoom";
import { initDetachedWindowGeometry, initWindowGeometry } from "../shared/window-geometry";
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

const HYDRATION_RETRY_DELAYS = [50, 100, 250, 500, 1000] as const;

const TerminalApp: Component<TerminalAppProps> = (props) => {
  const unlisteners: UnlistenFn[] = [];
  let disposed = false;
  let shortcutHandler: ((event: KeyboardEvent) => void) | null = null;
  let cleanupZoom: (() => void) | null = null;
  let cleanupGeometry: (() => void) | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | null = null;
  let retryAttempt = 0;
  let hydrationInFlightGeneration: number | null = null;
  let observedConnection: TransportConnectionState = {
    state: "disconnected",
    generation: -1,
  };
  const mountDisposed = Symbol("terminalAppMountDisposed");

  const isCentral = () => !props.lockedSessionId;
  const isHomeShown = createMemo(
    () => !!(props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible),
  );
  const shouldMountTerminal = createMemo(() => {
    if (props.lockedSessionId) return terminalStore.activeSessionId === props.lockedSessionId;
    return (
      terminalStore.selectionMode === "live" &&
      terminalStore.bindingState !== "unavailable"
    );
  });

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

  const cancelRetry = (): void => {
    if (retryTimer) {
      clearTimeout(retryTimer);
      retryTimer = null;
    }
  };

  const reconcileSelection = async (
    selection: SessionSelection,
    generation: number,
    allowEqualReconnect: boolean,
  ): Promise<boolean> => {
    if (disposed) return false;
    const accepted = terminalStore.reserveSelection(
      selection,
      generation,
      allowEqualReconnect,
    );
    if (!accepted) return false;

    cancelRetry();
    retryAttempt = 0;
    voiceRecorder.revokeLiveBinding();
    if (selection.mode !== "live") return true;

    try {
      const sessions = await SessionAPI.list();
      if (disposed || !terminalStore.matchesSelection(selection, generation)) return true;
      const session = sessions.find((candidate) => candidate.id === selection.id);
      if (!session || typeof session.status !== "string") {
        voiceRecorder.revokeSession(selection.id);
        terminalStore.markUnavailable(selection, generation);
        return true;
      }
      terminalStore.bindLive(selection, generation, session);
    } catch (error) {
      if (!disposed && terminalStore.matchesSelection(selection, generation)) {
        console.error("[selection] Failed to resolve live session metadata:", error);
        voiceRecorder.revokeSession(selection.id);
        terminalStore.markUnavailable(selection, generation);
      }
    }
    return true;
  };

  const scheduleHydrationRetry = (generation: number): void => {
    if (disposed || retryTimer || observedConnection.state !== "connected") return;
    if (
      observedConnection.generation !== generation ||
      terminalStore.awaitingHydrationGeneration !== generation
    ) {
      return;
    }
    const delay = HYDRATION_RETRY_DELAYS[Math.min(retryAttempt, HYDRATION_RETRY_DELAYS.length - 1)];
    retryAttempt += 1;
    retryTimer = setTimeout(() => {
      retryTimer = null;
      void requestHydration(generation);
    }, delay);
  };

  const requestHydration = async (generation: number): Promise<void> => {
    if (
      disposed ||
      observedConnection.state !== "connected" ||
      observedConnection.generation !== generation ||
      hydrationInFlightGeneration === generation ||
      !terminalStore.beginHydration(generation)
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
        terminalStore.awaitingHydrationGeneration !== generation
      ) {
        return;
      }
      await reconcileSelection(selection, generation, true);
    } catch (error) {
      if (
        disposed ||
        observedConnection.state !== "connected" ||
        observedConnection.generation !== generation ||
        terminalStore.awaitingHydrationGeneration !== generation
      ) {
        return;
      }
      if (isSelectionCoordinatorBusyError(error)) {
        scheduleHydrationRetry(generation);
      } else {
        terminalStore.cancelHydration(generation);
        terminalStore.suspendLiveBinding();
        voiceRecorder.revokeLiveBinding();
        console.error("[selection] Selection hydration failed:", error);
      }
    } finally {
      if (hydrationInFlightGeneration === generation) {
        hydrationInFlightGeneration = null;
      }
    }
  };

  const applyConnectionState = (state: TransportConnectionState): void => {
    if (disposed) return;
    if (state.generation < observedConnection.generation) return;
    if (
      state.generation === observedConnection.generation &&
      state.state === observedConnection.state
    ) {
      return;
    }
    const generationChanged = state.generation !== observedConnection.generation;
    observedConnection = { ...state };
    terminalStore.observeConnection(state);
    cancelRetry();
    retryAttempt = 0;
    if (state.state === "disconnected" || generationChanged) {
      voiceRecorder.revokeLiveBinding();
    }
    if (state.state === "disconnected") {
      return;
    }
    void requestHydration(state.generation);
  };

  const loadLockedSession = async (): Promise<void> => {
    try {
      const sessions = await SessionAPI.list();
      if (disposed) return;
      const session = sessions.find((candidate) => candidate.id === props.lockedSessionId);
      if (session && typeof session.status === "string") {
        terminalStore.bindLockedSession(session);
      } else {
        terminalStore.clearLockedSession();
      }
    } catch (error) {
      if (!disposed) {
        console.error("[detached] Failed to hydrate locked session:", error);
        terminalStore.clearLockedSession();
      }
    }
  };

  onMount(async () => {
    try {
    shortcutHandler = registerShortcuts();

    if (isCentral()) {
      await register(
        onSessionSwitched((selection, deliveryGeneration) => {
          void reconcileSelection(selection, deliveryGeneration, false);
        }),
      );
      if (disposed) return;
      await register(onTransportConnectionState(applyConnectionState));
      if (disposed) return;
    }

    await register(
      onSessionDestroyed(({ id }) => {
        voiceRecorder.revokeSession(id);
        if (props.lockedSessionId && id === props.lockedSessionId) {
          if (isTauri) {
            void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
              getCurrentWindow().destroy(),
            ).catch((error: unknown) => {
              console.error("[detached] Failed to close destroyed session window:", error);
            });
          }
          return;
        }
        if (isCentral()) terminalStore.safetySuspendDestroyed(id);
      }),
    );
    if (disposed) return;

    if (isCentral()) {
      applyConnectionState(getTransportConnectionState());
    } else {
      await loadLockedSession();
    }

    if (isTauri && props.detached && props.lockedSessionId) {
      const sessionId = props.lockedSessionId;
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      if (disposed) return;
      const win = getCurrentWindow();
      addUnlistener(
        await win.onCloseRequested(async (event) => {
          event.preventDefault();
          try {
            await WindowAPI.attach(sessionId);
          } catch (error) {
            console.error("[detached] attach failed during close; destroying window:", error);
            try {
              await win.destroy();
            } catch (destroyError) {
              console.error("[detached] fallback window destroy failed:", destroyError);
            }
          }
        }),
      );
      if (disposed) return;
    }

    if (!props.embedded) {
      cleanupZoom = await initZoom(props.detached ? "detached" : "terminal");
      if (disposed) {
        cleanupZoom();
        cleanupZoom = null;
        return;
      }
      cleanupGeometry = props.detached && props.lockedSessionId
        ? await initDetachedWindowGeometry(props.lockedSessionId)
        : await initWindowGeometry("terminal");
      if (disposed) {
        cleanupGeometry();
        cleanupGeometry = null;
        return;
      }
    }

    await settingsStore.load();
    if (disposed) return;
    if (!props.embedded) {
      document.documentElement.classList.toggle(
        "light-theme",
        !!settingsStore.current?.themeLight,
      );
    }

    await register(
      onSessionRenamed(({ id, name }) => terminalStore.renameBoundSession(id, name)),
    );

    const normalizePathForCompare = (path: string): string => {
      let normalized = path;
      if (normalized.startsWith("\\\\?\\")) normalized = normalized.slice(4);
      else if (normalized.startsWith("//?/")) normalized = normalized.slice(4);
      return normalized.replace(/\\/g, "/").toLowerCase();
    };
    await register(
      onWorkgroupTaskUpdated((data) => {
        if (data.source === "poll") {
          const targetId = props.lockedSessionId ?? terminalStore.activeSessionId;
          if (!targetId || !data.sessionIds.includes(targetId)) return;
          terminalStore.setActiveWorkgroupTask(data.task);
        } else if (data.source === "manual") {
          const workgroupRoot = data.workgroupRoot;
          const cwd = terminalStore.activeWorkingDirectory;
          if (!cwd || !workgroupRoot) return;
          const cwdNormalized = normalizePathForCompare(cwd);
          const rootNormalized = normalizePathForCompare(workgroupRoot);
          if (
            cwdNormalized === rootNormalized ||
            cwdNormalized.startsWith(`${rootNormalized}/`)
          ) {
            terminalStore.setActiveWorkgroupTask(data.task);
          }
        }
      }),
    );

    if (!props.embedded) {
      await register(
        onThemeChanged(({ light }) =>
          document.documentElement.classList.toggle("light-theme", light),
        ),
      );
    }
    } catch (error) {
      if (error !== mountDisposed) throw error;
    }
  });

  onCleanup(() => {
    disposed = true;
    cancelRetry();
    terminalStore.cancelHydration();
    unlisteners.forEach((unlisten) => unlisten());
    if (shortcutHandler) unregisterShortcuts(shortcutHandler);
    cleanupZoom?.();
    cleanupGeometry?.();
  });

  const emptyMessage = createMemo(() => {
    if (props.lockedSessionId) return "Session closed";
    if (terminalStore.selectionMode === "dormant") {
      return "Session exited. Wake it from the sidebar.";
    }
    if (terminalStore.selectionMode === "live") return "Session unavailable";
    return "No active session";
  });

  return (
    <div
      class="terminal-layout"
      data-ac-testid="terminal.root"
      data-ac-role="surface"
      data-ac-state={
        terminalStore.activeSessionId
          ? "active"
          : terminalStore.selectionMode === "dormant"
            ? "dormant"
            : terminalStore.bindingState
      }
    >
      <Show when={!props.embedded}>
        <Titlebar detached={props.detached} lockedSessionId={props.lockedSessionId} />
      </Show>
      <Show when={!terminalStore.activeIsRootAgent}>
        <WorkgroupTask />
      </Show>
      <LastPrompt sessionId={props.lockedSessionId} />
      <div class="terminal-content-area">
        <Show
          when={shouldMountTerminal()}
          fallback={
            <div class="terminal-empty" data-ac-testid="terminal.empty" data-ac-role="status">
              <span>{emptyMessage()}</span>
            </div>
          }
        >
          <TerminalView lockedSessionId={props.lockedSessionId} />
          <Show when={!props.lockedSessionId && terminalStore.bindingState === "pending"}>
            <div class="terminal-empty terminal-pending" data-ac-testid="terminal.pending">
              <span>Loading session…</span>
            </div>
          </Show>
        </Show>
      </div>
      <StatusBar detached={props.detached} />
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
