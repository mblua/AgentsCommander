import { Component, createEffect, createMemo, createSignal, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import iconUrl from "../../../src-tauri/icons/64x64.png";
import { isTauri } from "../../shared/platform";
import {
  SessionAPI,
  SettingsAPI,
  TelegramAPI,
  WindowAPI,
  emitOpenSettings,
} from "../../shared/ipc";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../../shared/stores/settings";
import ContextBadge from "./ContextBadge";
import { contextBadgeConfigured } from "./session-context";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import type { Session, TelegramBotConfig } from "../../shared/types";
import { sessionProfileBadge } from "../../shared/profile-utils";
import AgentPickerModal, { type AgentPickerSelection } from "./AgentPickerModal";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";
import { rootAgentCodingAgentAction } from "./root-agent-action";
import { TelegramIcon } from "./TelegramIcon";
import DetachIcon from "./DetachIcon";
import ReattachIcon from "./ReattachIcon";
import { sessionDotClass } from "./session-status";
import SessionRowMenu from "./context-menu/SessionRowMenu";
import {
  addToGroupSpec,
  clearTaskTitleSpec,
  closeSpec,
  deleteAgentSpec,
  detachSpec,
  matrixFolderSpec,
  openFolderSpec,
  reposSpec,
  taskTitleSpec,
  telegramSpec,
} from "./context-menu/session-row-menu-specs";

const RootAgentBanner: Component = () => {
  const [busy, setBusy] = createSignal(false);
  // #1871 - the menu is open iff menuPos() !== null. menuEpoch is a plain let,
  // not a signal, exactly as replicaCtxMenuEpoch is in ProjectPanel: nothing
  // renders from it.
  const [menuPos, setMenuPos] = createSignal<{ x: number; y: number } | null>(null);
  const [menuTelegramBots, setMenuTelegramBots] =
    createSignal<{ epoch: number; sessionId: string; bots: TelegramBotConfig[] } | null>(null);
  let menuEpoch = 0;
  const [showAgentPicker, setShowAgentPicker] = createSignal(false);
  const [showBotMenu, setShowBotMenu] = createSignal(false);
  const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);

  const rootSession = createMemo<Session | undefined>(() =>
    sessionsStore.sessions.find((s) => s.isRootAgent)
  );

  const isActive = createMemo(() => {
    const r = rootSession();
    return !!r && sessionsStore.activeId === r.id;
  });

  const hasLivePty = createMemo(() => {
    const r = rootSession();
    return !!r && typeof r.status === "string";
  });

  createEffect(() => {
    const root = rootSession();
    if (root && typeof root.status !== "string") voiceRecorder.revokeSession(root.id);
  });

  const dotClass = createMemo(() => {
    return sessionDotClass(rootSession());
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
  const agentLabel = createMemo(() => {
    const r = rootSession();
    if (!r) return null;
    if (r.agentLabel) return r.agentLabel;
    if (!r.agentId) return null;
    return settingsStore.current?.agents?.find((a) => a.id === r.agentId)?.label ?? null;
  });

  const ctxVisible = () =>
    contextBadgeConfigured(settingsStore.current?.agents, rootSession()?.agentId);
  const ctxPercent = () => {
    const r = rootSession();
    return r ? sessionsStore.contextPercentBySessionId[r.id] : undefined;
  };

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
  const focusTerminal = async (sessionId: string) => {
    if (!isTauri) return;
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const detachedLabel = `terminal-${sessionId.replace(/-/g, "")}`;
    const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
    if (!detachedWin) {
      await WindowAPI.ensureTerminal();
    }
  };

  // #1871 - transcribed from advanceReplicaCtxMenuEpoch / closeReplicaCtxMenu
  // in ProjectPanel. An expanded bot list never survives an epoch change.
  const advanceMenuEpoch = () => {
    menuEpoch += 1;
    setMenuTelegramBots(null);
    return menuEpoch;
  };
  const closeMenu = () => {
    setMenuPos(null);
    advanceMenuEpoch();
  };

  const handleContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setShowBotMenu(false); // mutual exclusion with the row's bot chooser
    advanceMenuEpoch(); // on-open reset; both live models do it
    setMenuPos({ x: e.clientX, y: e.clientY });
  };

  const handleClick = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      const r = rootSession();
      if (!r) {
        const session = await SessionAPI.createRootAgent();
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
    if (!hasLivePty()) return;
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
    if (!hasLivePty()) return;
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
    if (!hasLivePty()) return;
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
    closeMenu();
    if (!hasLivePty()) return;
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
    if (!hasLivePty()) return;
    const r = rootSession();
    if (!r) return;
    await TelegramAPI.attach(r.id, botId);
  };

  const handleClose = async (e: MouseEvent) => {
    e.stopPropagation();
    const r = rootSession();
    if (!r || busy()) return;
    setBusy(true);
    voiceRecorder.revokeSession(r.id);
    try {
      await SessionAPI.destroy(r.id);
    } catch (error) {
      console.error("[RootAgentBanner] Failed to close Root Agent:", error);
    } finally {
      setBusy(false);
    }
  };

  // #1871 - menu-side handlers. The catalogue's onSelect takes no MouseEvent,
  // so the two row handlers that take one (handleOpenExplorer, handleClose)
  // are copied here minus their e.stopPropagation(); the originals keep
  // serving the row buttons. SessionRowMenu dismisses before it invokes
  // onSelect, so none of these calls closeMenu() itself.
  const menuOpenFolder = async () => {
    const r = rootSession();
    if (!r) return;
    try {
      await WindowAPI.openInExplorer(r.workingDirectory);
    } catch (err) {
      console.error("Failed to open explorer:", err);
    }
  };

  const menuClose = async () => {
    const r = rootSession();
    if (!r || busy()) return;
    setBusy(true);
    voiceRecorder.revokeSession(r.id);
    try {
      await SessionAPI.destroy(r.id);
    } catch (error) {
      console.error("[RootAgentBanner] Failed to close Root Agent:", error);
    } finally {
      setBusy(false);
    }
  };

  const menuOpenRepo = async (sourcePath: string) => {
    try {
      await WindowAPI.openInExplorer(sourcePath);
    } catch (e) {
      console.error("Failed to open repo folder:", e);
    }
  };

  type MenuTelegramToken = {
    epoch: number;
    sessionId: string;
    startingBridge: ReturnType<typeof bridge>;
  };

  // Mirrors currentReplicaTelegramInvocation (ProjectPanel): after the await
  // the menu must still be open, on the same session, still live, and the
  // bridge must not have changed underneath.
  const menuTelegramStillCurrent = (tok: MenuTelegramToken): boolean =>
    menuEpoch === tok.epoch &&
    menuPos() !== null &&
    hasLivePty() &&
    rootSession()?.id === tok.sessionId &&
    (bridge() ?? null) === (tok.startingBridge ?? null);

  const menuTelegram = async () => {
    const r = rootSession();
    const startingBridge = bridge();
    const startedLive = hasLivePty() && !!r;
    const epoch = advanceMenuEpoch(); // invalidates any in-flight fetch
    if (!r || !startedLive) {
      closeMenu();
      return;
    }
    const tok: MenuTelegramToken = { epoch, sessionId: r.id, startingBridge };
    try {
      if (startingBridge) {
        closeMenu();
        await TelegramAPI.detach(r.id);
        return;
      }
      const settings = await SettingsAPI.get();
      if (!menuTelegramStillCurrent(tok)) return; // stale: publish nothing
      const bots = settings.telegramBots || [];
      if (bots.length === 0) {
        closeMenu(); // parity with ProjectPanel: zero bots closes the menu
        return;
      }
      if (bots.length === 1) {
        closeMenu();
        await TelegramAPI.attach(r.id, bots[0].id);
        return;
      }
      setShowBotMenu(false); // mutual exclusion with the row's bot chooser
      setMenuTelegramBots({ epoch, sessionId: r.id, bots }); // menu deliberately stays open
    } catch (e) {
      console.error("[RootAgentBanner] telegram menu action failed:", e);
    }
  };

  const menuSelectBot = async (botId: string) => {
    const r = rootSession();
    const choices = menuTelegramBots();
    if (
      !choices ||
      choices.epoch !== menuEpoch ||
      menuPos() === null ||
      !r ||
      !hasLivePty() ||
      choices.sessionId !== r.id ||
      (bridge() ?? null) !== null
    ) {
      return; // parity with ProjectPanel's bot-select guard
    }
    const sessionId = r.id;
    closeMenu();
    try {
      await TelegramAPI.attach(sessionId, botId);
    } catch (e) {
      console.error("[RootAgentBanner] telegram bot attach failed:", e);
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
          <img
            src={iconUrl}
            class="root-agent-avatar-img"
            alt=""
            draggable={false}
          />
        </div>
        <div class="root-agent-text">
          <span class="root-agent-title">Agent's Commander</span>
          <Show when={!isRecording() && !isProcessing() && !isAutoExecuting() && !isTypingWarning() && !voiceRecorder.micError()}>
            <span class="root-agent-subtitle">
              {subtitle()}
              {/* #624 / #1167 - coding-agent badge, mirroring SessionItem. Both
                  now emit the Coordinator row's class pair
                  (.ac-discovery-badge.agent), so the badge is one constant style
                  everywhere: no data-agent, no `running`, and identical for a
                  live and a dormant root. root-agent-badge adds only the inline
                  placement inside the uppercase .root-agent-subtitle. */}
              <Show when={agentLabel()}>
                {(label) => (
                  <span class="ac-discovery-badge agent root-agent-badge">{label()}</span>
                )}
              </Show>
              <Show when={profileBadge()}>
                {(badge) => <span class="profile-badge root-profile-badge">{badge()}</span>}
              </Show>
              <Show when={ctxVisible()}>
                <ContextBadge
                  percent={ctxPercent()}
                  testId="rootAgent.contextBadge"
                />
              </Show>
              {/* #592 - drift reload for the Root Agent (loaded profile cell no
                  longer matches its current config). Reuses the restart path, which
                  re-stamps the content hash and clears the flag. */}
              <Show when={rootSession()?.profileOutdated}>
                <ProfileOutdatedBadge onReload={() => void handleRestart()} />
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
              title={isDetached() ? "Re-attach session" : "Detach session"}
              data-ac-testid="rootAgent.detachToggle"
              data-ac-role="button"
              data-ac-state={isDetached() ? "detached" : "attached"}
            >
              {isDetached() ? <ReattachIcon /> : <DetachIcon />}
            </button>

            <button
              class={`session-item-telegram ${bridge() ? "active" : ""}`}
              onClick={handleTelegramClick}
              title={bridge() ? `Detach Telegram: ${bridge()!.botLabel}` : "Attach Telegram"}
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
          </Show>
          <button
            class="session-item-close"
            onClick={(event) => void handleClose(event)}
            title="Close session (Ctrl+Shift+W)"
            data-ac-testid="rootAgent.destroy"
            data-ac-role="button"
          >
            &#x2715;
          </button>
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
      {/* #1871 - every item is derived from data by a *Spec helper. A literal
          `false` on any key is a defect: the day the root carries repo data,
          the same reposSpec expression turns the entries on with no code
          change. root-menu-derived-caps.test.tsx pins this record exactly. */}
      <SessionRowMenu
        open={menuPos() !== null}
        x={menuPos()?.x ?? 0}
        y={menuPos()?.y ?? 0}
        testIdPrefix="rootAgent"
        onDismiss={closeMenu}
        caps={{
          restart: { onSelect: () => void handleRestart(), disabled: !rootSession() },
          codingAgent: { onSelect: handleCodingAgent },
          openFolder: openFolderSpec(rootSession(), { onSelect: () => void menuOpenFolder() }),
          repos: reposSpec(rootSession()?.gitRepos ?? [], {
            browseItems: () => [],
            onOpenRepo: (p) => void menuOpenRepo(p),
            onOpenBrowse: () => {},
          }),
          matrixFolder: matrixFolderSpec(undefined),
          close: closeSpec(rootSession(), { onSelect: () => void menuClose() }),
          deleteAgent: deleteAgentSpec(undefined),
          detach: detachSpec(hasLivePty(), isDetached(), () => void handleContextDetachToggle()),
          telegram: telegramSpec(hasLivePty() ? rootSession() : undefined, {
            on: !!bridge(),
            bridgeColor: bridge()?.color ?? null,
            bots: menuTelegramBots()?.bots ?? null,
            onSelect: () => void menuTelegram(),
            onSelectBot: (id) => void menuSelectBot(id),
          }),
          addToGroup: addToGroupSpec(undefined),
          editTaskTitle: taskTitleSpec(undefined),
          clearTaskTitle: clearTaskTitleSpec(undefined),
        }}
      />
    </>
  );
};

export default RootAgentBanner;
