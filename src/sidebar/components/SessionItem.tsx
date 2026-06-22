import { Component, createSignal, Show, For, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { Session, SessionStatus, TelegramBotConfig, RepoMatch } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI, AgentCreatorAPI, emitOpenSettings } from "../../shared/ipc";
import { extractProjectName } from "../../shared/path-extractors";
import { isTauri } from "../../shared/platform";
import { bridgesStore } from "../stores/bridges";
import { sessionsStore } from "../stores/sessions";
import { requestCoordinatorClose } from "../stores/coordinator-close";
import { settingsStore } from "../../shared/stores/settings";
import { centralViewStore } from "../../main/stores/centralView";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import OpenAgentModal from "./OpenAgentModal";
import AgentPickerModal from "./AgentPickerModal";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";
import { TelegramIcon } from "./TelegramIcon";
import { profileDisplayLabel, sessionProfileBadge } from "../../shared/profile-utils";

function statusClass(status: SessionStatus): string {
  if (typeof status === "string") return status;
  return "exited";
}

const CONTEXT_MENU_VIEWPORT_MARGIN = 8;

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
  originProject?: string;
}> = (props) => {
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [showAgentModal, setShowAgentModal] = createSignal(false);
  const [showCodingAgentPicker, setShowCodingAgentPicker] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);
  const [showContextMenu, setShowContextMenu] = createSignal(false);
  const [contextMenuPos, setContextMenuPos] = createSignal({ x: 0, y: 0 });
  let contextMenuEl: HTMLDivElement | undefined;

  const bridge = () => bridgesStore.getBridge(props.session.id);
  const sessionAgentLabel = () => {
    if (props.session.agentLabel) return props.session.agentLabel;
    if (!props.session.agentId) return null;
    return settingsStore.current?.agents?.find((a) => a.id === props.session.agentId)?.label ?? null;
  };
  const profileBadge = () => sessionProfileBadge(props.session);
  // #548: unify with the ProjectPanel quick-access tooltip — resolve the name of
  // the EFFECTIVE profile via the SAME shared resolver (no second resolver, no
  // badge-string parsing). Plain function, matching profileBadge.
  const profileBadgeTitle = () => {
    const badge = profileBadge();
    if (!badge) return undefined;
    const cfg = settingsStore.current?.codingAgentProfiles;
    const letter = props.session.effectiveProfile || props.session.requestedProfile;
    // Graceful degrade: before settings load there is no cfg; keep today's
    // letter-only tooltip rather than dropping it. Once loaded, show the resolved
    // name (same shape as the ProjectPanel quick-access tooltip).
    if (!cfg || !letter) return `Profile ${badge}`;
    return profileDisplayLabel(cfg, settingsStore.current?.agents ?? [], props.session.agentId, letter);
  };
  const sessionHasLivePty = () => !isInactive() && typeof props.session.status === "string";
  const isRecording = () => voiceRecorder.recordingSessionId() === props.session.id;
  const isProcessing = () => voiceRecorder.processingSessionId() === props.session.id;
  const isAutoExecuting = () => voiceRecorder.autoExecuteSessionId() === props.session.id;
  const isTypingWarning = () => voiceRecorder.typingWarnSessionId() === props.session.id;

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (!settingsStore.voiceEnabled) {
      emitOpenSettings("integrations").catch(console.error);
      return;
    }
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
    // #587 — cover an embedded RM with the terminal even when switch_session
    // no-ops on the already-active session (which emits no session_switched).
    centralViewStore.showTerminal();
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

  const isDetached = () => sessionsStore.isDetached(props.session.id);

  const handleDetachToggle = async (e: MouseEvent) => {
    e.stopPropagation();
    try {
      if (isDetached()) {
        await WindowAPI.attach(props.session.id);
      } else {
        await WindowAPI.detach(props.session.id);
      }
    } catch (err) {
      console.error("detach/attach toggle failed:", err);
    }
  };

  const handleContextDetachToggle = async () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    try {
      if (isDetached()) {
        await WindowAPI.attach(props.session.id);
      } else {
        await WindowAPI.detach(props.session.id);
      }
    } catch (err) {
      console.error("context detach/attach toggle failed:", err);
    }
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    // #588 route through the shared helper: closing a coordinator from the
    // session list marks + (settings-gated) cascades + confirms when busy. For a
    // non-coordinator this is identical to the previous SessionAPI.destroy.
    void requestCoordinatorClose(props.session);
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
      window.addEventListener("keydown", dismiss as any);
    });
  };

  const restartSession = async (agentId?: string, requestedProfile?: string | null) => {
    setShowContextMenu(false);
    cleanupContextMenu();
    try {
      await SessionAPI.restart(
        props.session.id,
        agentId ? { agentId, requestedProfile } : undefined,
      );
    } catch (e) {
      console.error("Failed to restart session:", e);
    }
  };

  const handleRestart = async () => {
    await restartSession();
  };

  const handleCodingAgentRestart = () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    setShowCodingAgentPicker(true);
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
   *  Project AC Root paths: "agent-name@origin-project" (e.g. "code-reviewer@phi_phibridge")
   *  Other paths: "parentFolder/name" (last 2 segments)
   *
   *  Project AC Root parsing is delegated to extractProjectName. The innermost
   *  .ac segment wins, matching the titlebar helpers. */
  const displayName = () => {
    const wd = props.session.workingDirectory;
    if (wd) {
      const pathProject = extractProjectName(wd);
      if (pathProject) {
        const projectFolder = props.originProject || pathProject;
        const normalized = wd.replace(/\\/g, "/").replace(/\/+$/, "");
        const parts = normalized.split("/");
        const agentDir = parts[parts.length - 1].replace(/^__?agent_/, "");
        return `${agentDir}@${projectFolder}`;
      }
      const normalized = wd.replace(/\\/g, "/").replace(/\/+$/, "");
      const parts = normalized.split("/");
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
      data-ac-testid={`session.${props.session.id}`}
      data-ac-role="button"
      data-ac-state={props.isActive ? "active" : isInactive() ? "inactive" : "idle"}
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
          <Show when={sessionAgentLabel() || (props.session.isCoordinator && !isInactive() && props.session.gitRepos.length > 0)}>
            <div class="session-item-meta">
              <Show when={sessionAgentLabel()}>
                {(agentLabel) => (
                  <span
                    class={`agent-badge ${sessionHasLivePty() ? "running" : ""}`}
                    data-agent={agentLabel()}
                  >
                    {agentLabel()}
                  </span>
                )}
              </Show>
              <Show when={profileBadge()}>
                {(badge) => (
                  <span
                    class="profile-badge"
                    title={profileBadgeTitle()}
                  >
                    {badge()}
                  </span>
                )}
              </Show>
              <Show when={props.session.profileOutdated}>
                <ProfileOutdatedBadge onReload={() => void restartSession()} />
              </Show>
              <Show when={props.session.isCoordinator && !isInactive() && props.session.gitRepos.length > 0}>
                <div class="session-item-branches">
                  <For each={props.session.gitRepos}>
                    {(repo) => (
                      <div
                        class="session-item-branch"
                        title={`${repo.label}${repo.branch ? `/${repo.branch}` : ""}`}
                      >
                        {repo.label}{repo.branch ? `/${repo.branch}` : ""}
                      </div>
                    )}
                  </For>
                </div>
              </Show>
            </div>
          </Show>
        </Show>
      </div>
      <Show when={!isInactive()}>
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
        <button
          class="session-item-explorer"
          onClick={handleOpenExplorer}
          title="Open folder in explorer"
        >
          &#x1F4C2;
        </button>
        <button
          class="session-item-detach"
          classList={{ attached: isDetached() }}
          onClick={handleDetachToggle}
          title={isDetached() ? "Re-attach to main window" : "Open in new window"}
          innerHTML={isDetached() ? "&#x2934;" : "&#x29C9;"}
          data-ac-testid={`session.${props.session.id}.detachToggle`}
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
          data-ac-testid={`session.${props.session.id}.destroy`}
          data-ac-role="button"
        >
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
      {showCodingAgentPicker() && (
        <Portal>
          <AgentPickerModal
            sessionName={props.session.name}
            agentPath={props.session.workingDirectory}
            currentAgentId={props.session.agentId}
            currentRequestedProfile={props.session.requestedProfile}
            onSelect={async (selection) => {
              setShowCodingAgentPicker(false);
              await restartSession(selection.agent.id, selection.requestedProfile);
            }}
            onClose={() => setShowCodingAgentPicker(false)}
          />
        </Portal>
      )}
      {showContextMenu() && (
        <Portal>
          <div
            class="session-context-menu"
            ref={contextMenuEl}
            style={{ left: `${contextMenuPos().x}px`, top: `${contextMenuPos().y}px` }}
            onClick={(e) => e.stopPropagation()}
            data-ac-testid={`session.${props.session.id}.menu`}
            data-ac-role="menu"
          >
            <button
              class="session-context-option context-option-danger"
              onClick={handleRestart}
              data-ac-testid={`session.${props.session.id}.restart`}
              data-ac-role="menuitem"
            >
              Restart Session
            </button>
            <button
              class="session-context-option"
              onClick={handleCodingAgentRestart}
            >
              Coding Agent
            </button>
            <div class="context-separator" />
            <button
              class="session-context-option"
              onClick={handleContextDetachToggle}
              data-ac-testid={`session.${props.session.id}.menu.detachToggle`}
              data-ac-role="menuitem"
              data-ac-state={isDetached() ? "detached" : "attached"}
            >
              {isDetached() ? "Re-attach to main" : "Open in new window"}
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
