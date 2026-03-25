import { Component, createSignal, Show, For } from "solid-js";
import type { Session, SessionStatus, TelegramBotConfig, RepoMatch } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI } from "../../shared/ipc";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
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

/** Match a shell command to a detected agent name */
function shellMatchesAgent(shell: string, agent: string): boolean {
  const s = shell.toLowerCase();
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
}> = (props) => {
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [showAgentModal, setShowAgentModal] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);

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
    const detachedLabel = `terminal-${props.session.id.replace(/-/g, "")}`;
    const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
    if (!detachedWin) {
      (await WebviewWindow.getByLabel("terminal"))?.setFocus();
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


  const isInactive = () => props.session.id.startsWith("inactive-");

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""} ${isInactive() ? "inactive-member" : ""}`}
      onClick={isInactive() ? undefined : handleClick}
    >
      <div
        class={`session-item-status ${isInactive() ? "offline" : props.session.waitingForInput ? "waiting" : statusClass(props.session.status)}`}
      />
      <div class="session-item-info">
        <div class="session-item-name" onDblClick={handleDoubleClick}>
          {props.session.name.includes("/") ? (
            <>
              <span class="name-prefix">{props.session.name.slice(0, props.session.name.lastIndexOf("/") + 1)}</span>
              {props.session.name.slice(props.session.name.lastIndexOf("/") + 1)}
            </>
          ) : props.session.name}
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
                  const isRunning = !isInactive() && shellMatchesAgent(props.session.shell, agent);
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
        <OpenAgentModal
          initialRepo={repoForModal()}
          onClose={() => setShowAgentModal(false)}
        />
      )}
    </div>
  );
};

export default SessionItem;
