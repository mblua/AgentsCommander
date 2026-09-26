import { Component, createSignal, Show, For, onCleanup, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import type { Session, TelegramBotConfig, RepoMatch } from "../../shared/types";
import { SessionAPI, TelegramAPI, SettingsAPI, WindowAPI, emitOpenSettings } from "../../shared/ipc";
import { extractProjectName } from "../../shared/path-extractors";
import { isTauri } from "../../shared/platform";
import { bridgesStore } from "../stores/bridges";
import { sessionsStore } from "../stores/sessions";
import { requestCoordinatorClose } from "../stores/coordinator-close";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import OpenAgentModal from "./OpenAgentModal";
import AgentPickerModal from "./AgentPickerModal";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";
import ContextBadge from "./ContextBadge";
import { contextBadgeConfigured } from "./session-context";
import { quotaChipAttrs } from "./agent-quota";
import { TelegramIcon } from "./TelegramIcon";
import DetachIcon from "./DetachIcon";
import ReattachIcon from "./ReattachIcon";
import { profileDisplayLabel, sessionProfileBadge, sessionTierBadge } from "../../shared/profile-utils";
import { sessionDotClass } from "./session-status";

const CONTEXT_MENU_VIEWPORT_MARGIN = 8;

export interface SessionContextExtraAction {
  label: string;
  class?: string;
  icon?: JSX.Element;
  testId?: string;
  onSelect: () => void;
}

export function sessionRowState(isActive: boolean, inactive: () => boolean): string {
  if (isActive) return "active";
  return inactive() ? "inactive" : "idle";
}

export function micButtonClass(
  recording: boolean,
  processing: boolean,
  micError: string | null,
  voiceEnabled: boolean,
): string {
  return `session-item-mic ${recording ? "recording" : ""} ${processing ? "processing" : ""} ${micError ? "error" : ""} ${!voiceEnabled ? "disabled" : ""}`;
}

/** Thunks keep the old ternary chain's lazy reads (and tracked deps). */
export function micButtonTitle(
  voiceEnabled: boolean,
  recording: () => boolean,
  processing: () => boolean,
  micError: () => string | null,
): string {
  if (!voiceEnabled) return "Enable voice-to-text in Settings and set a Gemini API key to use this.";
  if (recording()) return "Stop recording";
  if (processing()) return "Transcribing...";
  return micError() || "Voice to text";
}

export function repoBranchLabel(repo: { label: string; branch?: string | null }): string {
  return `${repo.label}${repo.branch ? `/${repo.branch}` : ""}`;
}

export function detachUi(detached: boolean): { title: string; state: string; menuLabel: string } {
  return detached
    ? { title: "Re-attach session", state: "detached", menuLabel: "Re-attach session" }
    : { title: "Detach session", state: "attached", menuLabel: "Detach session" };
}

export function telegramUi(bridge: { botLabel: string; color: string } | null | undefined): {
  title: string;
  style: JSX.CSSProperties;
} {
  return bridge
    ? { title: `Detach Telegram: ${bridge.botLabel}`, style: { color: bridge.color } }
    : { title: "Attach Telegram", style: {} };
}

export function sessionDisplayName(
  session: { workingDirectory: string; name: string },
  originProject: () => string | undefined,
): string {
  const wd = session.workingDirectory;
  if (wd) {
    const pathProject = extractProjectName(wd);
    if (pathProject) {
      const projectFolder = originProject() || pathProject;
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
    return parts[parts.length - 1] || session.name;
  }
  return session.name;
}

// #1730 - the bare identity for agent-name-chip: exactly the non-dim text the
// deleted .session-item-name div rendered, minus the "@project" suffix that
// sessionDisplayName() appends in its extractProjectName branch (the only branch
// that appends one). Split on "/" first: the fallback branch returns
// "parent/folder", whose last segment may itself contain "@". Strip the suffix by
// LENGTH, not by re-splitting on "@" and not by removing an unanchored "@project"
// substring, so an "@" inside either half cannot mis-split. This pair of
// expressions mirrors the pair inside sessionDisplayName(), and must move with it.
export function sessionChipName(
  full: string,
  wd: string,
  originProject: () => string | undefined,
): string {
  if (wd) {
    const pathProject = extractProjectName(wd);
    if (pathProject) {
      const projectFolder = originProject() || pathProject;
      return full.slice(0, full.length - projectFolder.length - 1);
    }
  }
  const slash = full.lastIndexOf("/");
  return slash >= 0 ? full.slice(slash + 1) : full;
}

const SessionItem: Component<{
  session: Session;
  isActive: boolean;
  originProject?: string;
  extraContextAction?: SessionContextExtraAction;
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
  const ctxVisible = () =>
    contextBadgeConfigured(settingsStore.current?.agents, props.session.agentId);
  const ctxPercent = () => sessionsStore.contextPercentBySessionId[props.session.id];
  const quotaUsed = () => {
    const agentId = props.session.agentId;
    return agentId ? sessionsStore.weeklyQuotaUsedByAgentId[agentId] : undefined;
  };
  const profileBadgeTitle = () => {
    const badge = profileBadge();
    if (!badge) return undefined;
    const cfg = settingsStore.current?.codingAgentProfiles;
    const letter = props.session.effectiveProfile || props.session.requestedProfile;
    if (!cfg || !letter) return `Profile ${badge}`;
    return profileDisplayLabel(cfg, settingsStore.current?.agents ?? [], props.session.agentId, letter);
  };
  const sessionHasLivePty = () => !isInactive() && typeof props.session.status === "string";
  const isRecording = () => voiceRecorder.recordingSessionId() === props.session.id;
  const isProcessing = () => voiceRecorder.processingSessionId() === props.session.id;
  const isAutoExecuting = () => voiceRecorder.autoExecuteSessionId() === props.session.id;
  const isTypingWarning = () => voiceRecorder.typingWarnSessionId() === props.session.id;
  // #1730 - the voice indicators replace the status chips, but never the identity.
  const voiceQuiet = () =>
    !isRecording() && !isProcessing() && !isAutoExecuting() && !isTypingWarning() &&
    !voiceRecorder.micError();

  const handleMicClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (!sessionHasLivePty()) return;
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
    if (!sessionHasLivePty()) return;
    const b = bridge();
    if (b) {
      await TelegramAPI.detach(props.session.id);
    } else {
      await openBotChooser();
    }
  };

  const openBotChooser = async () => {
    const settings = await SettingsAPI.get();
    const bots = settings.telegramBots || [];
    if (bots.length === 1) {
      await TelegramAPI.attach(props.session.id, bots[0].id);
    } else if (bots.length > 1) {
      setAvailableBots(bots);
      setShowBotMenu(true);
    }
  };

  const handleBotSelect = async (botId: string) => {
    setShowBotMenu(false);
    if (!sessionHasLivePty()) return;
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

  const repoMenuEntries = () =>
    props.session.gitRepos.filter(
      (repo) => typeof repo.sourcePath === "string" && repo.sourcePath.trim().length > 0,
    );

  const handleOpenRepoExplorer = async (sourcePath: string) => {
    setShowContextMenu(false);
    cleanupContextMenu();
    try {
      await WindowAPI.openInExplorer(sourcePath);
    } catch (err) {
      console.error("Failed to open repo folder:", err);
    }
  };

  const isDetached = () => sessionsStore.isDetached(props.session.id);

  const toggleDetach = async (logLabel: string) => {
    try {
      if (isDetached()) {
        await WindowAPI.attach(props.session.id);
      } else {
        await WindowAPI.detach(props.session.id);
      }
    } catch (err) {
      console.error(logLabel, err);
    }
  };

  const handleDetachToggle = async (e: MouseEvent) => {
    e.stopPropagation();
    if (!sessionHasLivePty()) return;
    await toggleDetach("detach/attach toggle failed:");
  };

  const handleContextDetachToggle = async () => {
    setShowContextMenu(false);
    cleanupContextMenu();
    if (!sessionHasLivePty()) return;
    await toggleDetach("context detach/attach toggle failed:");
  };

  const handleClose = (e: MouseEvent) => {
    e.stopPropagation();
    void requestCoordinatorClose(props.session);
  };

  let dismissContextMenu: EventListener | null = null;

  const cleanupContextMenu = () => {
    if (dismissContextMenu) {
      window.removeEventListener("click", dismissContextMenu);
      window.removeEventListener("contextmenu", dismissContextMenu);
      window.removeEventListener("keydown", dismissContextMenu);
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
    const dismiss: EventListener = (ev) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      setShowContextMenu(false);
      cleanupContextMenu();
    };
    dismissContextMenu = dismiss;
    setTimeout(() => {
      positionContextMenu(e.clientX, e.clientY);
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss);
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

  const isInactive = () => props.session.id.startsWith("inactive-");

  // #2271 - the Co-managed sidecar is keyed by session id and deliberately
  // outside Session, so a list refresh cannot wipe it.
  const isComanaged = () =>
    sessionsStore.comanagedBySessionId[props.session.id] ?? false;

  const displayName = () => sessionDisplayName(props.session, () => props.originProject);
  // #1730 - sessionChipName mirrors sessionDisplayName; keep the pair together.
  const chipName = () =>
    sessionChipName(displayName(), props.session.workingDirectory, () => props.originProject);
  const chipTitle = () => `${displayName()}\n${props.session.workingDirectory}`;

  return (
    <div
      class={`session-item session-item-enter ${props.isActive ? "active" : ""} ${isInactive() ? "inactive-member" : ""}`}
      onClick={isInactive() ? undefined : handleClick}
      onContextMenu={isInactive() ? undefined : handleContextMenu}
      data-ac-testid={`session.${props.session.id}`}
      data-ac-role="button"
      data-ac-state={sessionRowState(props.isActive, isInactive)}
    >
      <div
        class={`session-item-status ${sessionDotClass(props.session, { inactive: isInactive() })}${isComanaged() ? " comanaged" : ""}`}
        data-ac-comanaged={isComanaged() ? "true" : "false"}
        title={isComanaged() ? "Co-managed" : undefined}
      />
      <div class="session-item-info">

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

        <div class="session-item-meta">
          <Show when={voiceQuiet() && props.session.profileOutdated}>
            <ProfileOutdatedBadge onReload={() => void restartSession()} />
          </Show>
          <span class="agent-name-chip" onDblClick={handleDoubleClick} title={chipTitle()}>
            {chipName()}
          </span>
          <Show when={voiceQuiet()}>
            {/* #1167 - one constant coding-agent style for every sidebar row.
                This emits the same class pair the workgroup/Coordinator rows
                emit (ProjectPanel.tsx:2271), so the two cannot drift apart
                again. No data-agent and no `running`: the badge must not vary
                by label or by PTY liveness. Liveness is still carried by the
                row status dot and .session-item.inactive-member. */}
            <Show when={sessionAgentLabel()}>{(agentLabel) => (
              <span {...quotaChipAttrs(agentLabel(), quotaUsed())}>{agentLabel()}</span>
            )}</Show>
            <Show when={profileBadge()}>{(badge) => (
              <span class="profile-badge" title={profileBadgeTitle()}>{badge()}</span>
            )}</Show>
            <Show when={sessionTierBadge(props.session)}>{(tier) => (
              <span
                class="profile-badge profile-badge--tier"
                title={tier().title}
                data-ac-testid={`session.${props.session.id}.tierBadge`}
              >{tier().text}</span>
            )}</Show>
            <Show when={ctxVisible()}>
              <ContextBadge percent={ctxPercent()} testId={`session.${props.session.id}.contextBadge`} />
            </Show>
            <Show when={props.session.isCoordinator && !isInactive() && props.session.gitRepos.length > 0}>
              <div class="session-item-branches">
                <For each={props.session.gitRepos}>{(repo) => (
                  <div class="session-item-branch" title={repoBranchLabel(repo)}>
                    {repoBranchLabel(repo)}
                  </div>
                )}</For>
              </div>
            </Show>
          </Show>
        </div>
      </div>
      <Show when={!isInactive()}>
        <Show when={sessionHasLivePty()}>
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
            class={micButtonClass(
              isRecording(),
              isProcessing(),
              voiceRecorder.micError(),
              settingsStore.voiceEnabled,
            )}
            onClick={handleMicClick}
            title={micButtonTitle(
              settingsStore.voiceEnabled,
              isRecording,
              isProcessing,
              voiceRecorder.micError,
            )}
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
        <Show when={sessionHasLivePty()}>
          <button
            class="session-item-detach"
            onClick={handleDetachToggle}
            title={detachUi(isDetached()).title}
            data-ac-testid={`session.${props.session.id}.detachToggle`}
            data-ac-role="button"
            data-ac-state={detachUi(isDetached()).state}
          >
            {isDetached() ? <ReattachIcon /> : <DetachIcon />}
          </button>
          <button
            class={`session-item-telegram ${bridge() ? "active" : ""}`}
            onClick={handleTelegramClick}
            title={telegramUi(bridge()).title}
            style={telegramUi(bridge()).style}
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
        </Show>
        <button
          class="session-item-close"
          onClick={handleClose}
          title="Close session (Ctrl+Shift+W)"
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
              data-ac-testid={`session.${props.session.id}.coding-agent`}
              data-ac-role="menuitem"
            >
              Coding Agent
            </button>
            <Show when={props.session.isCoordinator && !isInactive() && repoMenuEntries().length > 0}>
              <div class="context-separator" />
              <For each={repoMenuEntries()}>
                {(repo, index) => (
                  <button
                    class="session-context-option session-context-repo-option"
                    onClick={() => void handleOpenRepoExplorer(repo.sourcePath)}
                    title={`Open repo folder: ${repo.sourcePath}`}
                    data-ac-testid={`session.${props.session.id}.menu.repo.${index()}`}
                    data-ac-role="menuitem"
                  >
                    <svg
                      class="session-context-repo-icon"
                      viewBox="0 0 16 16"
                      aria-hidden="true"
                    >
                      <path
                        fill="currentColor"
                        d="M1.75 4.25A1.75 1.75 0 0 1 3.5 2.5h3.1c.46 0 .9.18 1.22.5l.9.9h3.78A1.75 1.75 0 0 1 14.25 5.65v5.1a1.75 1.75 0 0 1-1.75 1.75h-9A1.75 1.75 0 0 1 1.75 10.75v-6.5Z"
                      />
                    </svg>
                    <span class="session-context-repo-label">{repo.label}</span>
                  </button>
                )}
              </For>
            </Show>
            <Show when={props.extraContextAction}>
              {(action) => (
                <>
                  <div class="context-separator" />
                  <button
                    class={`session-context-option ${action().class ?? ""}`}
                    onClick={() => {
                      setShowContextMenu(false);
                      cleanupContextMenu();
                      action().onSelect();
                    }}
                    data-ac-testid={action().testId}
                    data-ac-role="menuitem"
                  >
                    {action().icon}
                    {action().label}
                  </button>
                </>
              )}
            </Show>
            <Show when={sessionHasLivePty()}>
              <div class="context-separator" />
              <button
                class="session-context-option"
                onClick={handleContextDetachToggle}
                data-ac-testid={`session.${props.session.id}.menu.detachToggle`}
                data-ac-role="menuitem"
                data-ac-state={detachUi(isDetached()).state}
              >
                {detachUi(isDetached()).menuLabel}
              </button>
            </Show>
          </div>
        </Portal>
      )}
    </div>
  );
};

export default SessionItem;
