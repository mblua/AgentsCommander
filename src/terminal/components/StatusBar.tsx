import { Component, Show, createMemo, onCleanup } from "solid-js";
import { terminalStore } from "../stores/terminal";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import { PtyAPI, WindowAPI, emitOpenSettings } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";

const MIC_DISABLED_TITLE =
  "Enable voice-to-text in Settings and set a Gemini API key to use this.";

const StatusBar: Component<{ detached?: boolean }> = (props) => {
  let mouseUpHandler: (() => void) | null = null;

  const isRecording = () => !!voiceRecorder.recordingSessionId();
  const isProcessing = () => !!voiceRecorder.processingSessionId();

  const fullCommand = createMemo(() => {
    const shell = terminalStore.activeShell;
    const args = terminalStore.activeShellArgs;
    if (!shell || args === null || args === undefined) return "";
    return args.length > 0 ? `${shell} ${args.join(" ")}` : shell;
  });

  const handleMicDown = (e: MouseEvent) => {
    e.preventDefault();
    if (!settingsStore.voiceEnabled) {
      emitOpenSettings("integrations").catch(console.error);
      return;
    }
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId || isProcessing()) return;

    void voiceRecorder.start(sessionId);

    // Use document mouseup so release works anywhere on screen
    mouseUpHandler = () => {
      voiceRecorder.stop();
      cleanup();
    };
    document.addEventListener("mouseup", mouseUpHandler);
  };

  const cleanup = () => {
    if (mouseUpHandler) {
      document.removeEventListener("mouseup", mouseUpHandler);
      mouseUpHandler = null;
    }
  };

  const handleCancelRecording = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    cleanup();
    voiceRecorder.cancel();
  };

  onCleanup(cleanup);

  // #1171 - opens the activity window scoped to this session, or focuses and re-scopes it
  // when it is already open. Tauri-only: the web client renders this same StatusBar but has
  // no such window and no arm for the command, and a button that returns a raw error is
  // worse than an absent one.
  const handleOpenWatchers = () => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) return;
    WindowAPI.openWatchers(sessionId).catch(console.error);
  };

  const handleClearInput = () => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) return;
    // Ctrl+U: kills line backward in readline/bash/most coding agents
    const encoder = new TextEncoder();
    PtyAPI.write(sessionId, encoder.encode("\x15"));
  };

  return (
    <div class="status-bar">
      <div class="status-bar-left">
        <Show when={props.detached}>
          <div class="status-bar-item">
            <span class="status-bar-detached">DETACHED</span>
          </div>
        </Show>
        <Show when={fullCommand()}>
          <div class="status-bar-item status-bar-command">
            <span class="status-bar-accent" title={fullCommand()}>
              {fullCommand()}
            </span>
          </div>
        </Show>
        <Show when={isRecording()}>
          <div class="status-bar-item status-bar-recording">
            <span class="status-bar-rec-dot" />
            <span>{formatRecordingTime(voiceRecorder.recordingSeconds())}</span>
          </div>
        </Show>
        <Show when={isProcessing()}>
          <div class="status-bar-item status-bar-processing">
            <span class="status-bar-spinner" />
            <span>Transcribing...</span>
          </div>
        </Show>
        <Show when={voiceRecorder.micError()}>
          <div class="status-bar-item status-bar-error">
            {voiceRecorder.micError()}
          </div>
        </Show>
      </div>
      <Show when={terminalStore.activeSessionId}>
        <div class="status-bar-actions">
          <Show when={isRecording()}>
            <button
              class="status-bar-btn status-bar-btn-mic-cancel"
              onClick={handleCancelRecording}
              title="Cancel recording"
            >
              &#x2715;
            </button>
          </Show>
          <button
            class={`status-bar-btn status-bar-btn-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${!settingsStore.voiceEnabled ? "disabled" : ""}`}
            onMouseDown={handleMicDown}
            title={
              !settingsStore.voiceEnabled
                ? MIC_DISABLED_TITLE
                : isRecording()
                  ? "Release to stop"
                  : isProcessing()
                    ? "Transcribing..."
                    : "Hold to record (Ctrl+Shift+R)"
            }
            disabled={isProcessing()}
          >
            &#x1F399;
          </button>
          <Show when={isTauri}>
            <button
              class="status-bar-btn"
              onClick={handleOpenWatchers}
              title="Watcher activity"
              data-ac-testid="statusBar.watchers"
              data-ac-role="button"
            >
              &#x1F4E1;
            </button>
          </Show>
          <button
            class="status-bar-btn status-bar-btn-clear"
            onClick={handleClearInput}
            title="Clear agent input (Ctrl+U)"
          >
            &#x232B;
          </button>
        </div>
      </Show>
    </div>
  );
};

export default StatusBar;
