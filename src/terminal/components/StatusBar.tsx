import { Component, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { terminalStore } from "../stores/terminal";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder, formatRecordingTime } from "../../shared/voice-recorder";
import { PtyAPI, WindowAPI, emitOpenSettings } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import type { TypingHoldSnapshot } from "../../shared/types";
import TypingHoldIcon from "./TypingHoldIcon";

const MIC_DISABLED_TITLE =
  "Enable voice-to-text in Settings and set a Gemini API key to use this.";

// #2337 - the backend owns the padlock; this is only the read cadence.
const TYPING_HOLD_POLL_MS = 500;

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

  // #2337 - the snapshot is stored with the session it came from. Rendering only
  // when the ids agree keeps a slow fetch for tab A from painting over tab B and
  // makes the closed state and pending count per session.
  const [typingHold, setTypingHold] = createSignal<{
    sessionId: string;
    snapshot: TypingHoldSnapshot;
  } | null>(null);
  // #2337 - pending is per session: a toggle for tab A must not disable tab B's
  // padlock (and must not swallow B's clicks) while it is in flight.
  const [typingHoldPending, setTypingHoldPending] = createSignal<ReadonlySet<string>>(
    new Set(),
  );
  // #2337 - one order for every writer of `typingHold`: polls and toggles share
  // this sequence, so a poll that left before a toggle cannot land after it and
  // repaint the pre-toggle state.
  let typingHoldSeq = 0;

  const markTypingHoldPending = (sessionId: string, pending: boolean) => {
    setTypingHoldPending((previous) => {
      const next = new Set(previous);
      if (pending) next.add(sessionId);
      else next.delete(sessionId);
      return next;
    });
  };
  const typingHoldIsPending = (sessionId: string | null) =>
    !!sessionId && typingHoldPending().has(sessionId);

  const typingHoldSnapshot = (): TypingHoldSnapshot | null => {
    const state = typingHold();
    return state && state.sessionId === terminalStore.activeSessionId
      ? state.snapshot
      : null;
  };
  const typingHoldClosed = () => !!typingHoldSnapshot()?.closed;
  const typingHoldTitle = () => {
    const snapshot = typingHoldSnapshot();
    // No snapshot yet: make no claim about a count.
    if (!snapshot) return "Hold message delivery to this session";
    return snapshot.closed
      ? `Release held messages and resume delivery (${snapshot.heldCount} held)`
      : `Hold message delivery to this session (${snapshot.heldCount} held)`;
  };

  // #2337 - poll while this bar is mounted. The effect re-runs on every active
  // session change: cleanup drops the timer AND the previous session's late
  // responses (the `cancelled` flag), so a slow fetch for tab A can never paint
  // over tab B. A failed poll keeps the last known snapshot and retries on the
  // next tick; it never fabricates a release. Desktop only: the web client has
  // no arm for these commands and hides the control instead (see the render).
  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    if (!isTauri || !sessionId) return;
    let cancelled = false;
    const fetchSnapshot = async () => {
      const seq = ++typingHoldSeq;
      try {
        const snapshot = await PtyAPI.getTypingHold(sessionId);
        if (cancelled || seq !== typingHoldSeq) return;
        setTypingHold({ sessionId, snapshot });
      } catch {
        // Keep the last known state; the next tick retries.
      }
    };
    void fetchSnapshot();
    const timer = setInterval(() => void fetchSnapshot(), TYPING_HOLD_POLL_MS);
    onCleanup(() => {
      cancelled = true;
      clearInterval(timer);
    });
  });

  const handleToggleTypingHold = async () => {
    // Captured at click: a tab switch mid-request must not retarget the toggle.
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId || typingHoldIsPending(sessionId)) return;
    markTypingHoldPending(sessionId, true);
    try {
      const snapshot = await PtyAPI.toggleTypingHold(sessionId);
      // Retire every poll that started before this result, so none of them can
      // land afterwards and repaint the state the toggle just replaced.
      typingHoldSeq += 1;
      if (terminalStore.activeSessionId === sessionId) {
        setTypingHold({ sessionId, snapshot });
      }
    } catch {
      // A failed toggle must not claim one; refetch the authoritative state.
      try {
        const snapshot = await PtyAPI.getTypingHold(sessionId);
        typingHoldSeq += 1;
        if (terminalStore.activeSessionId === sessionId) {
          setTypingHold({ sessionId, snapshot });
        }
      } catch {
        // Keep the last known state; the poll retries on its next tick.
      }
    } finally {
      markTypingHoldPending(sessionId, false);
    }
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
          <Show when={isTauri}>
            <button
              class="status-bar-btn status-bar-btn-typing-hold"
              classList={{ closed: typingHoldClosed() }}
              onClick={handleToggleTypingHold}
              disabled={typingHoldIsPending(terminalStore.activeSessionId)}
              title={typingHoldTitle()}
              aria-label={typingHoldTitle()}
              aria-pressed={typingHoldClosed()}
              data-ac-testid="statusBar.typingHold"
              data-ac-role="button"
            >
              <TypingHoldIcon
                closed={typingHoldClosed()}
                class="status-bar-typing-hold-icon"
              />
              <Show when={typingHoldClosed()}>
                <span class="status-bar-hold-count">
                  {typingHoldSnapshot()!.heldCount}
                </span>
              </Show>
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
