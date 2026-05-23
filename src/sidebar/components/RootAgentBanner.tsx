import { Component, createMemo, createSignal, Show, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { isTauri } from "../../shared/platform";
import { SessionAPI, WindowAPI } from "../../shared/ipc";
import { sessionsStore } from "../stores/sessions";
import type { Session, SessionStatus } from "../../shared/types";
import AgentPickerModal from "./AgentPickerModal";

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const CONTEXT_MENU_VIEWPORT_MARGIN = 8;

const RootAgentBanner: Component = () => {
  const [busy, setBusy] = createSignal(false);
  const [showContextMenu, setShowContextMenu] = createSignal(false);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  const [showAgentPicker, setShowAgentPicker] = createSignal(false);
  let contextMenuEl: HTMLDivElement | undefined;

  const rootSession = createMemo<Session | undefined>(() =>
    sessionsStore.sessions.find((s) => s.isRootAgent)
  );

  const isActive = createMemo(() => {
    const r = rootSession();
    return !!r && sessionsStore.activeId === r.id;
  });

  const dotClass = createMemo(() => {
    const r = rootSession();
    if (!r) return "offline";
    if (r.pendingReview) return "pending";
    if (r.waitingForInput) return "waiting";
    return statusClass(r.status);
  });

  const subtitle = createMemo(() => {
    const r = rootSession();
    if (!r) return "Root Agent";
    if (typeof r.status !== "string") return "Exited — click to wake";
    return "Root Agent";
  });

  const focusTerminal = async (sessionId: string) => {
    if (!isTauri) return;
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const detachedLabel = `terminal-${sessionId.replace(/-/g, "")}`;
    const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
    if (!detachedWin) {
      await WindowAPI.ensureTerminal();
    }
  };

  let dismissContextMenu: ((ev?: Event) => void) | null = null;

  const cleanupContextMenu = () => {
    if (dismissContextMenu) {
      window.removeEventListener("click", dismissContextMenu);
      window.removeEventListener("contextmenu", dismissContextMenu);
      window.removeEventListener("keydown", dismissContextMenu as EventListener);
      dismissContextMenu = null;
    }
  };

  onCleanup(cleanupContextMenu);

  const positionContextMenu = (x: number, y: number) => {
    if (!contextMenuEl) return;
    const { width, height } = contextMenuEl.getBoundingClientRect();
    const maxX = Math.max(
      CONTEXT_MENU_VIEWPORT_MARGIN,
      window.innerWidth - width - CONTEXT_MENU_VIEWPORT_MARGIN
    );
    const maxY = Math.max(
      CONTEXT_MENU_VIEWPORT_MARGIN,
      window.innerHeight - height - CONTEXT_MENU_VIEWPORT_MARGIN
    );
    setContextMenuPos({
      x: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, x), maxX),
      y: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, y), maxY),
    });
  };

  const handleContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    cleanupContextMenu();
    setContextMenuPos({ x: e.clientX, y: e.clientY });
    setShowContextMenu(true);
    const dismiss = (ev?: Event) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      setShowContextMenu(false);
      cleanupContextMenu();
    };
    dismissContextMenu = dismiss;
    setTimeout(() => {
      positionContextMenu(e.clientX, e.clientY);
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss as EventListener);
    });
  };

  const handleClick = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      const r = rootSession();
      if (!r) {
        const session = await SessionAPI.createRootAgent();
        await SessionAPI.switch(session.id);
        await focusTerminal(session.id);
      } else if (typeof r.status !== "string") {
        const session = await SessionAPI.restart(r.id, { skipAutoResume: false });
        await SessionAPI.switch(session.id);
        await focusTerminal(session.id);
      } else {
        await SessionAPI.switch(r.id);
        await focusTerminal(r.id);
      }
    } catch (e) {
      console.error("[RootAgentBanner] click failed:", e);
    } finally {
      setBusy(false);
    }
  };

  const handleRestart = async () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    if (busy()) return;
    const r = rootSession();
    if (!r) return;
    setBusy(true);
    try {
      const session = await SessionAPI.restart(r.id);
      await SessionAPI.switch(session.id);
      await focusTerminal(session.id);
    } catch (e) {
      console.error("[RootAgentBanner] restart failed:", e);
    } finally {
      setBusy(false);
    }
  };

  const handleCodingAgent = () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    setShowAgentPicker(true);
  };

  const handleAgentSelected = async (agentId: string) => {
    setShowAgentPicker(false);
    if (busy()) return;
    setBusy(true);
    try {
      const r = rootSession();
      if (!r) {
        const session = await SessionAPI.createRootAgent({ agentId });
        await SessionAPI.switch(session.id);
        await focusTerminal(session.id);
      } else {
        const session = await SessionAPI.restart(r.id, { agentId });
        await SessionAPI.switch(session.id);
        await focusTerminal(session.id);
      }
    } catch (e) {
      console.error("[RootAgentBanner] coding-agent change failed:", e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button
        class="root-agent-banner"
        classList={{ active: isActive() }}
        onClick={handleClick}
        onContextMenu={handleContextMenu}
        disabled={busy()}
        title={
          rootSession()
            ? "Open Root Agent session (right-click for options)"
            : "Create Root Agent session"
        }
      >
        <div class={`session-item-status ${dotClass()}`} />
        <div class="root-agent-avatar">
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
          >
            <path d="M12 2L2 7l10 5 10-5-10-5z" />
            <path d="M2 17l10 5 10-5" />
            <path d="M2 12l10 5 10-5" />
          </svg>
        </div>
        <div class="root-agent-text">
          <span class="root-agent-title">Agent's Commander</span>
          <span class="root-agent-subtitle">{subtitle()}</span>
        </div>
      </button>
      <Show when={showAgentPicker()}>
        <Portal>
          <AgentPickerModal
            sessionName={rootSession()?.name ?? "Root Agent"}
            onSelect={(agent) => handleAgentSelected(agent.id)}
            onClose={() => setShowAgentPicker(false)}
          />
        </Portal>
      </Show>
      <Show when={showContextMenu()}>
        <Portal>
          <div
            class="session-context-menu"
            ref={contextMenuEl}
            style={{
              left: `${contextMenuPos().x}px`,
              top: `${contextMenuPos().y}px`,
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              class="session-context-option context-option-danger"
              onClick={handleRestart}
              disabled={!rootSession()}
            >
              Restart Session
            </button>
            <button class="session-context-option" onClick={handleCodingAgent}>
              Coding Agent
            </button>
          </div>
        </Portal>
      </Show>
    </>
  );
};

export default RootAgentBanner;
