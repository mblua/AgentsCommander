import { Component, createMemo, createSignal, Show, For, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { isTauri } from "../../shared/platform";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  WindowAPI,
  AgentCreatorAPI,
  emitOpenSettings,
} from "../../shared/ipc";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import type { Session, SessionStatus, TelegramBotConfig } from "../../shared/types";
import { sessionProfileBadge } from "../../shared/profile-utils";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";
import { rootAgentCodingAgentAction } from "./root-agent-action";
import { TelegramIcon } from "./TelegramIcon";

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
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);
  let contextMenuEl: HTMLDivElement | undefined;

  const rootSession = createMemo<Session | undefined>(() =>
    sessionsStore.sessions.find((s) => s.isRootAgent)
  );

  const isActive = createMemo(() => {
    const r = rootSession();
    return !!r && sessionsStore.activeId === r.id;
  });

  // Dormant root has status as `{ exited: number }` (not a string). Backend
  // marks root exited/dormant instead of removing the record so the banner can
  // re-wake — but PTY-dependent actions (mic, detach, telegram, close) must be
  // hidden because there is no live PTY to receive input or attach to.
  const hasLivePty = createMemo(() => {
    const r = rootSession();
    return !!r && typeof r.status === "string";
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
  const profileBadge = createMemo(() => {
    const r = rootSession();
    return r ? sessionProfileBadge(r) : null;
  });

  const bridge = () => {
    const r = rootSession();
    return r ? bridgesStore.getBridge(r.id) : undefined;
  };
  const isRecording = () => {
    const r = rootSession();
    return !!r && voiceRecorder.recordingSessionId() === r.id;
  };
  const isProcessing = () => {
    const r = rootSession();
    return !!r && voiceRecorder.processingSessionId() === r.id;
  };
  const isAutoExecuting = () => {
    const r = rootSession();
    return !!r && voiceRecorder.autoExecuteSessionId() === r.id;
  };
  const isTypingWarning = () => {
    const r = rootSession();
    return !!r && voiceRecorder.typingWarnSessionId() === r.id;
  };
  const isDetached = () => {
    const r = rootSession();
    return !!r && sessionsStore.isDetached(r.id);
  };
  const hasClaude = () =>
    (settingsStore.current?.agents ?? []).some((a) =>
      a.command.toLowerCase().includes("claude")
    );

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
        // Hydrate the store: backend may reuse an existing live root and
        // therefore NOT emit session_created (see ReuseLive in
        // commands/session.rs). addSession upserts, so it's safe to call
        // even when session_created later races in.
        sessionsStore.addSession(session);
        await SessionAPI.switch(session.id);
        await focusTerminal(session.id);
      } else if (typeof r.status !== "string") {
        const session = await SessionAPI.restart(r.id, { skipAutoResume: false });
        sessionsStore.addSession(session);
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
      sessionsStore.addSession(session);
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

  const handleAgentSelected = async (selection: AgentPickerSelection) => {
    setShowAgentPicker(false);
    if (busy()) return;
    setBusy(true);
    try {
      const action = rootAgentCodingAgentAction(rootSession(), selection.agent.id);
      const session = action.kind === "create"
        ? await SessionAPI.createRootAgent({
            agentId: action.agentId,
            requestedProfile: selection.requestedProfile,
          })
        : await SessionAPI.restart(action.id, {
            agentId: action.agentId,
            requestedProfile: selection.requestedProfile,
            skipAutoResume: action.skipAutoResume,
          });
      sessionsStore.addSession(session);
      await SessionAPI.switch(session.id);
      await focusTerminal(session.id);
    } catch (e) {
      console.error("[RootAgentBanner] coding-agent change failed:", e);
    } finally {
      setBusy(false);
    }
  };

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (!settingsStore.voiceEnabled) {
      emitOpenSettings("integrations").catch(console.error);
      return;
    }
    const r = rootSession();
    if (!r) return;
    voiceRecorder.toggle(r.id);
  };

  const handleCancelRecording = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.cancel();
  };

  const handleCancelAutoExecute = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.cancelAutoExecute();
  };

  const handleOpenExplorer = async (e: MouseEvent) => {
    e.stopPropagation();
    const r = rootSession();
    if (!r) return;
    try {
      await WindowAPI.openInExplorer(r.workingDirectory);
    } catch (err) {
      console.error("Failed to open explorer:", err);
    }
  };

  const handleDetachToggle = async (e: MouseEvent) => {
    e.stopPropagation();
    const r = rootSession();
    if (!r) return;
    try {
      if (isDetached()) {
        await WindowAPI.attach(r.id);
      } else {
        await WindowAPI.detach(r.id);
      }
    } catch (err) {
      console.error("detach/attach toggle failed:", err);
    }
  };

  const handleContextDetachToggle = async () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    const r = rootSession();
    if (!r) return;
    try {
      if (isDetached()) {
        await WindowAPI.attach(r.id);
      } else {
        await WindowAPI.detach(r.id);
      }
    } catch (err) {
      console.error("context detach/attach toggle failed:", err);
    }
  };

  const handleTelegramClick = async (e: MouseEvent) => {
    e.stopPropagation();
    const r = rootSession();
    if (!r) return;
    const b = bridge();
    if (b) {
      await TelegramAPI.detach(r.id);
    } else {
      const settings = await SettingsAPI.get();
      const bots = settings.telegramBots || [];
      if (bots.length === 1) {
        await TelegramAPI.attach(r.id, bots[0].id);
      } else if (bots.length > 1) {
        setAvailableBots(bots);
        setShowBotMenu(true);
      }
    }
  };

  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    const r = rootSession();
    if (!r) return;
    await TelegramAPI.attach(r.id, botId);
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    const r = rootSession();
    if (!r) return;
    // Cancel local capture before destroy: backend marks the root dormant,
    // which hides the in-banner cancel control and leaves MediaRecorder +
    // mic stream running. cancel() also detaches onstop so a pending
    // transcription doesn't write to the now-dormant session.
    if (voiceRecorder.recordingSessionId() === r.id) {
      voiceRecorder.cancel();
    }
    // Root destroy is special: backend kills PTY and marks dormant rather
    // than removing the session record (see destroy_session_inner_with_options
    // in commands/session.rs). The banner stays visible and can re-wake.
    SessionAPI.destroy(r.id);
  };

  const handleExcludeClaudeMd = async (e: MouseEvent) => {
    e.stopPropagation();
    setShowContextMenu(false);
    cleanupContextMenu();
    const r = rootSession();
    if (!r) return;
    try {
      await AgentCreatorAPI.writeClaudeSettingsLocal(r.workingDirectory);
    } catch (err) {
      console.error("Failed to write claude settings:", err);
    }
  };

  return (
    <>
      <div
        class="root-agent-banner"
        classList={{ active: isActive(), disabled: busy() }}
        role="button"
        tabIndex={busy() ? -1 : 0}
        aria-disabled={busy()}
        aria-label={
          rootSession()
            ? typeof rootSession()!.status === "string"
              ? "Open Root Agent session"
              : "Wake Root Agent session"
            : "Create Root Agent session"
        }
        onClick={handleClick}
        onKeyDown={(e) => {
          // Only intercept keys aimed at the banner itself — when focus is on
          // a child action button, that button's native handler runs and the
          // event still bubbles here, so we must skip to avoid double-firing.
          if (e.currentTarget !== e.target) return;
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            handleClick();
          }
        }}
        onContextMenu={handleContextMenu}
        title={
          rootSession()
            ? "Open Root Agent session (right-click for options)"
            : "Create Root Agent session"
        }
        data-ac-testid="rootAgent.banner"
        data-ac-role="button"
        data-ac-state={rootSession() ? (hasLivePty() ? "live" : "dormant") : "missing"}
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
          <Show when={!isRecording() && !isProcessing() && !isAutoExecuting() && !isTypingWarning() && !voiceRecorder.micError()}>
            <span class="root-agent-subtitle">
              {subtitle()}
              <Show when={profileBadge()}>
                {(badge) => <span class="profile-badge root-profile-badge">{badge()}</span>}
              </Show>
            </span>
          </Show>

          <Show when={isRecording()}>
            <div class="session-item-voice-indicator recording">
              <div class="voice-dot" />
              <div class="voice-level-bar">
                <div
                  class="voice-level-fill"
                  style={{ width: `${Math.min(voiceRecorder.audioLevel() * 100 * 2.5, 100)}%` }}
                />
              </div>
              <span class="voice-time">{formatRecordingTime(voiceRecorder.recordingSeconds())}</span>
            </div>
          </Show>

          <Show when={isProcessing()}>
            <div class="session-item-voice-indicator processing">
              <div class="voice-spinner" />
              <span class="voice-processing-text">Transcribing...</span>
            </div>
          </Show>

          <Show when={isAutoExecuting()}>
            <div class="session-item-voice-indicator auto-execute">
              <span class="voice-countdown">{voiceRecorder.autoExecuteCountdown()}s</span>
              <span class="voice-execute-text">Auto-execute</span>
              <button class="voice-cancel-execute" onClick={handleCancelAutoExecute}>Cancel</button>
            </div>
          </Show>

          <Show when={isTypingWarning()}>
            <div class="session-item-voice-indicator warning">
              <span class="voice-warning-text">Typed during recording</span>
            </div>
          </Show>

          <Show when={voiceRecorder.micError() && (isRecording() || isProcessing() || rootSession())}>
            <div class="session-item-voice-indicator error">
              <span class="voice-error-text">{voiceRecorder.micError()}</span>
            </div>
          </Show>
        </div>

        <Show when={rootSession()}>
          {/* Cancel-recording is local cleanup (MediaRecorder + mic stream),
              not PTY-dependent — keep it visible even when the root is
              dormant so any in-flight recording can still be torn down. */}
          <Show when={isRecording()}>
            <button
              class="session-item-mic-cancel"
              onClick={handleCancelRecording}
              title="Cancel recording"
            >
              &#x2715;
            </button>
          </Show>
          <Show when={hasLivePty()}>
            <button
              class={`session-item-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${voiceRecorder.micError() ? "error" : ""} ${!settingsStore.voiceEnabled ? "disabled" : ""}`}
              onClick={handleMicClick}
              title={
                !settingsStore.voiceEnabled
                  ? "Enable voice-to-text in Settings and set a Gemini API key to use this."
                  : isRecording()
                    ? "Stop recording"
                    : isProcessing()
                      ? "Transcribing..."
                      : voiceRecorder.micError()
                        ? voiceRecorder.micError()!
                        : "Voice to text"
              }
            >
              &#x1F399;
            </button>
          </Show>
          <button
            class="session-item-explorer"
            onClick={handleOpenExplorer}
            title="Open folder in explorer"
          >
            &#x1F4C2;
          </button>
          <Show when={hasLivePty()}>
            <button
              class="session-item-detach"
              classList={{ attached: isDetached() }}
              onClick={handleDetachToggle}
              title={isDetached() ? "Re-attach to main window" : "Open in new window"}
              innerHTML={isDetached() ? "&#x2934;" : "&#x29C9;"}
              data-ac-testid="rootAgent.detachToggle"
              data-ac-role="button"
              data-ac-state={isDetached() ? "detached" : "attached"}
            />

            <Show when={bridge()}>
              <div
                class="session-item-bridge-dot"
                style={{ background: bridge()!.color }}
                title={`Telegram: ${bridge()!.botLabel}`}
              />
            </Show>
            <button
              class={`session-item-telegram ${bridge() ? "active" : ""}`}
              onClick={handleTelegramClick}
              title={bridge() ? "Detach Telegram" : "Attach Telegram"}
              style={bridge() ? { color: bridge()!.color } : {}}
            ><TelegramIcon /></button>
            <Show when={showBotMenu()}>
              <div class="session-item-bot-menu" onClick={(e) => e.stopPropagation()}>
                <For each={availableBots()}>
                  {(bot) => (
                    <button
                      class="session-item-bot-option"
                      onClick={() => handleBotSelect(bot.id)}
                    >
                      <span class="settings-color-dot" style={{ background: bot.color }} />
                      {bot.label}
                    </button>
                  )}
                </For>
              </div>
            </Show>
            <button
              class="session-item-close"
              onClick={handleClose}
              title="Close session"
              data-ac-testid="rootAgent.destroy"
              data-ac-role="button"
            >
              &#x2715;
            </button>
          </Show>
        </Show>
      </div>
      <Show when={showAgentPicker()}>
        <Portal>
          <AgentPickerModal
            sessionName={rootSession()?.name ?? "Root Agent"}
            agentPath={rootSession()?.workingDirectory}
            currentAgentId={rootSession()?.agentId}
            currentRequestedProfile={rootSession()?.requestedProfile}
            onSelect={handleAgentSelected}
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
            data-ac-testid="rootAgent.menu"
            data-ac-role="menu"
          >
            <button
              class="session-context-option context-option-danger"
              onClick={handleRestart}
              disabled={!rootSession()}
              data-ac-testid="rootAgent.restart"
              data-ac-role="menuitem"
            >
              Restart Session
            </button>
            <button class="session-context-option" onClick={handleCodingAgent}>
              Coding Agent
            </button>
            <Show when={rootSession()}>
              <Show when={hasLivePty()}>
                <div class="context-separator" />
                <button
                  class="session-context-option"
                  onClick={handleContextDetachToggle}
                  data-ac-testid="rootAgent.menu.detachToggle"
                  data-ac-role="menuitem"
                  data-ac-state={isDetached() ? "detached" : "attached"}
                >
                  {isDetached() ? "Re-attach to main" : "Open in new window"}
                </button>
              </Show>
              <Show when={hasClaude()}>
                <div class="context-separator" />
                <button class="session-context-option" onClick={handleExcludeClaudeMd}>
                  Exclude global CLAUDE.md
                </button>
              </Show>
            </Show>
          </div>
        </Portal>
      </Show>
    </>
  );
};

export default RootAgentBanner;
