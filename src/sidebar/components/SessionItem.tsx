import { Component, createSignal, Show, For, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { Session, SessionStatus, TelegramBotConfig, RepoMatch } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI, AgentCreatorAPI } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { bridgesStore } from "../stores/bridges";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import OpenAgentModal from "./OpenAgentModal";

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const AGENT_BADGES: Record<string, string> = {
  Claude: "CC",
  Codex: "CX",
  OpenCode: "OC",
  Cursor: "CU",
};

/** Match a shell command (+ args) to a detected agent name */
function shellMatchesAgent(shell: string, shellArgs: string[], agent: string): boolean {
  const s = `${shell} ${shellArgs.join(" ")}`.toLowerCase();
  switch (agent) {
    case "Claude": return s.includes("claude");
    case "Codex": return s.includes("codex");
    case "OpenCode": return s.includes("opencode");
    case "Cursor": return s.includes("cursor");
    default: return false;
  }
}

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
  originProject?: string;
}> = (props) => {
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [showAgentModal, setShowAgentModal] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);
  const [showContextMenu, setShowContextMenu] = createSignal(false);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });

  const bridge = () => bridgesStore.getBridge(props.session.id);
  const agentBadges = () => {
    const np = props.session.workingDirectory.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
    const repo = sessionsStore.repos.find((r) =>
      r.path.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "") === np
    );
    return repo?.agents ?? [];
  };
  const isRecording = () => voiceRecorder.recordingSessionId() === props.session.id;
  const isProcessing = () => voiceRecorder.processingSessionId() === props.session.id;
  const isAutoExecuting = () => voiceRecorder.autoExecuteSessionId() === props.session.id;
  const isTypingWarning = () => voiceRecorder.typingWarnSessionId() === props.session.id;

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.toggle(props.session.id);
  };

  const handleCancelRecording = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.cancel();
  };

  const handleCancelAutoExecute = (e: MouseEvent) => {
    e.stopPropagation();
    voiceRecorder.cancelAutoExecute();
  };

  const handleTelegramClick = async (e: MouseEvent) => {
    e.stopPropagation();
    const b = bridge();
    if (b) {
      await TelegramAPI.detach(props.session.id);
    } else {
      const settings = await SettingsAPI.get();
      const bots = settings.telegramBots || [];
      if (bots.length === 1) {
        await TelegramAPI.attach(props.session.id, bots[0].id);
      } else if (bots.length > 1) {
        setAvailableBots(bots);
        setShowBotMenu(true);
      }
    }
  };

  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    await TelegramAPI.attach(props.session.id, botId);
  };

  const handleClick = async () => {
    await SessionAPI.switch(props.session.id);
    if (isTauri) {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const detachedLabel = `terminal-${props.session.id.replace(/-/g, "")}`;
      const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
      if (!detachedWin) {
        await WindowAPI.ensureTerminal();
      }
    }
  };

  const handleDoubleClick = (e: MouseEvent) => {
    e.stopPropagation();
    setShowAgentModal(true);
  };

  const repoForModal = (): RepoMatch => {
    const np = props.session.workingDirectory.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
    const repo = sessionsStore.repos.find((r) =>
      r.path.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "") === np
    );
    return repo ?? { name: props.session.name, path: props.session.workingDirectory, agents: [] };
  };

  const handleOpenExplorer = async (e: MouseEvent) => {
    e.stopPropagation();
    try {
      await WindowAPI.openInExplorer(props.session.workingDirectory);
    } catch (err) {
      console.error("Failed to open explorer:", err);
    }
  };

  const handleDetach = (e: MouseEvent) => {
    e.stopPropagation();
    WindowAPI.detach(props.session.id);
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    SessionAPI.destroy(props.session.id);
  };

  /** True if any configured coding agent is Claude-based */
  const hasClaude = () =>
    (settingsStore.current?.agents ?? []).some((a) =>
      a.command.toLowerCase().includes("claude")
    );

  let dismissContextMenu: (() => void) | null = null;

  const cleanupContextMenu = () => {
    if (dismissContextMenu) {
      window.removeEventListener("click", dismissContextMenu);
      window.removeEventListener("contextmenu", dismissContextMenu);
      window.removeEventListener("keydown", dismissContextMenu as any);
      dismissContextMenu = null;
    }
  };

  onCleanup(cleanupContextMenu);

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
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss as any);
    });
  };

  const handleRestart = async () => {
    setShowContextMenu(false);
    try {
      await SessionAPI.restart(props.session.id);
    } catch (e) {
      console.error("Failed to restart session:", e);
    }
  };

  const handleExcludeClaudeMd = async (e: MouseEvent) => {
    e.stopPropagation();
    setShowContextMenu(false);
    try {
      await AgentCreatorAPI.writeClaudeSettingsLocal(props.session.workingDirectory);
    } catch (err) {
      console.error("Failed to write claude settings:", err);
    }
  };

  const isInactive = () => props.session.id.startsWith("inactive-");

  /** Derive short display name from workingDirectory.
   *  .ac-new paths: "agent-name@origin-project" (e.g. "code-reviewer@phi_phibridge")
   *  Other paths: "parentFolder/name" (last 2 segments)
   */
  const displayName = () => {
    const wd = props.session.workingDirectory;
    if (wd) {
      const normalized = wd.replace(/\\/g, "/").replace(/\/+$/, "");
      const parts = normalized.split("/");
      const acIdx = parts.indexOf(".ac-new");
      if (acIdx >= 1) {
        // Use origin project from identity resolution; fall back to path-derived project
        const projectFolder = props.originProject || parts[acIdx - 1];
        let agentDir = parts[parts.length - 1];
        agentDir = agentDir.replace(/^__?agent_/, "");
        return `${agentDir}@${projectFolder}`;
      }
      if (parts.length >= 2) {
        return parts.slice(-2).join("/");
      }
      return parts[parts.length - 1] || props.session.name;
    }
    return props.session.name;
  };

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""} ${isInactive() ? "inactive-member" : ""}`}
      onClick={isInactive() ? undefined : handleClick}
      onContextMenu={isInactive() ? undefined : handleContextMenu}
    >
      <div
        class={`session-item-status ${isInactive() ? "offline" : props.session.pendingReview ? "pending" : props.session.waitingForInput ? "waiting" : statusClass(props.session.status)}`}
      />
      <div class="session-item-info">
        <div class="session-item-name" onDblClick={handleDoubleClick} title={props.session.workingDirectory}>
          {displayName().includes("/") ? (
            <>
              <span class="name-prefix">{displayName().slice(0, displayName().lastIndexOf("/") + 1)}</span>
              {displayName().slice(displayName().lastIndexOf("/") + 1)}
            </>
          ) : displayName()}
        </div>

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

        <Show when={voiceRecorder.micError()}>
          <div class="session-item-voice-indicator error">
            <span class="voice-error-text">{voiceRecorder.micError()}</span>
          </div>
        </Show>

        <Show when={!isRecording() && !isProcessing() && !isAutoExecuting() && !isTypingWarning() && !voiceRecorder.micError()}>
          <Show when={agentBadges().length > 0}>
            <div class="session-item-agent-badges">
              <For each={agentBadges()}>
                {(agent) => {
                  const isRunning = !isInactive() && shellMatchesAgent(props.session.shell, props.session.shellArgs, agent);
                  return (
                    <span class={`agent-badge ${isRunning ? "running" : ""}`} data-agent={agent}>
                      {isRunning ? agent.toUpperCase() : (AGENT_BADGES[agent] || agent)}
                    </span>
                  );
                }}
              </For>
            </div>
          </Show>
          <Show when={!isInactive()}>
            <Show when={props.session.gitBranch}>
              <div class="session-item-branch" title={props.session.gitBranch!}>
                {props.session.gitBranch}
              </div>
            </Show>
          </Show>
        </Show>
      </div>
      <Show when={!isInactive()}>
        <Show when={settingsStore.voiceEnabled}>
          <Show when={isRecording()}>
            <button
              class="session-item-mic-cancel"
              onClick={handleCancelRecording}
              title="Cancel recording"
            >
              &#x2715;
            </button>
          </Show>
          <button
            class={`session-item-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${voiceRecorder.micError() ? "error" : ""}`}
            onClick={handleMicClick}
            title={isRecording() ? "Stop recording" : isProcessing() ? "Transcribing..." : voiceRecorder.micError() ? voiceRecorder.micError()! : "Voice to text"}
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
        <button
          class="session-item-detach"
          onClick={handleDetach}
          title="Detach to own window"
        >
          &#x29C9;
        </button>
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
        >
          T
        </button>
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
        <button class="session-item-close" onClick={handleClose} title="Close session">
          &#x2715;
        </button>
      </Show>
      {showAgentModal() && (
        <Portal>
          <OpenAgentModal
            initialRepo={repoForModal()}
            onClose={() => setShowAgentModal(false)}
          />
        </Portal>
      )}
      {showContextMenu() && (
        <Portal>
          <div
            class="session-context-menu"
            style={{ left: `${contextMenuPos().x}px`, top: `${contextMenuPos().y}px` }}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              class="session-context-option context-option-danger"
              onClick={handleRestart}
            >
              Restart Session
            </button>
            <Show when={hasClaude()}>
              <div class="context-separator" />
              <button class="session-context-option" onClick={handleExcludeClaudeMd}>
                Exclude global CLAUDE.md
              </button>
            </Show>
          </div>
        </Portal>
      )}
    </div>
  );
};

export default SessionItem;
