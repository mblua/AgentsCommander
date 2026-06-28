import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  onPtyOutput,
  onSessionDestroyed,
  onTerminalDetached,
} from "../../shared/ipc";
import { isBrowser, isTauri } from "../../shared/platform";
import { terminalStore } from "../stores/terminal";
import type { UnlistenFn } from "../../shared/transport";
import { updatePromptCapture } from "./prompt-input-capture";
import { createTerminalOptions } from "./terminal-options";
import "@xterm/xterm/css/xterm.css";

interface SessionTerminal {
  container: HTMLDivElement;
  terminal: Terminal;
  fitAddon: FitAddon;
  inputBuffer: string;
  snapshotReplayRequested: boolean;
  snapshotResizeSuppressed: boolean;
  hasRenderedOutput: boolean;
  replayStatus: HTMLDivElement;
}

interface TerminalViewProps {
  /**
   * If set, this TerminalView is inside a detached window locked to one
   * session. Disables the main-window pre-warm listener (plan §A2.3.G6).
   */
  lockedSessionId?: string;
}

// WebGL context budget: ~16 per document. Canvas fallback activates silently
// when the budget is exhausted (e.g. after ~16 concurrent sessions in main).
// See plan §DW.9.
const TerminalView: Component<TerminalViewProps> = (props) => {
  let hostRef!: HTMLDivElement;
  let activeSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;
  let unlistenTerminalDetached: UnlistenFn | null = null;

  const terminals = new Map<string, SessionTerminal>();

  const syncViewport = (sessionId: string, skipPtyResize = false) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    if (!skipPtyResize) {
      void PtyAPI.resize(sessionId, entry.terminal.cols, entry.terminal.rows);
    }
  };

  const scheduleViewportSync = (sessionId: string) => {
    requestAnimationFrame(() => {
      if (sessionId !== activeSessionId) {
        return;
      }

      syncViewport(sessionId);

      requestAnimationFrame(() => {
        if (sessionId === activeSessionId) {
          syncViewport(sessionId);
        }
      });
    });
  };

  const disposeSessionTerminal = (sessionId: string) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    entry.terminal.dispose();
    entry.container.remove();
    terminals.delete(sessionId);

    if (activeSessionId === sessionId) {
      activeSessionId = null;
    }
  };

  const writeAutomationInput = (value: string) => {
    if (!activeSessionId || !value) return;
    const terminalInput = value.replace(/\r?\n/g, "\r");

    const encoder = new TextEncoder();
    void PtyAPI.write(activeSessionId, encoder.encode(terminalInput));

    const entry = terminals.get(activeSessionId);
    if (!entry) return;

    for (const char of terminalInput) {
      const capture = updatePromptCapture(entry.inputBuffer, char);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(activeSessionId, capture.submittedPrompt);
      }
    }
  };

  const setReplayStatus = (entry: SessionTerminal, message: string | null) => {
    entry.replayStatus.textContent = message ?? "";
    entry.replayStatus.hidden = !message;
  };

  const writeTerminalBytes = (entry: SessionTerminal, data: Uint8Array) => {
    entry.hasRenderedOutput = true;
    setReplayStatus(entry, null);
    entry.terminal.write(data);
  };

  const resizeTerminalForSnapshot = (
    entry: SessionTerminal,
    cols: number,
    rows: number
  ) => {
    entry.snapshotResizeSuppressed = true;
    try {
      entry.terminal.resize(cols, rows);
    } finally {
      entry.snapshotResizeSuppressed = false;
    }
  };

  const replayNativeSnapshot = (sessionId: string, entry: SessionTerminal) => {
    if (!isTauri || entry.snapshotReplayRequested) {
      return;
    }

    entry.snapshotReplayRequested = true;

    void PtyAPI.getScreenSnapshot(sessionId)
      .then((snapshot) => {
        if (terminals.get(sessionId) !== entry) {
          return;
        }

        if (!snapshot || snapshot.data.length === 0) {
          if (!entry.hasRenderedOutput) {
            setReplayStatus(
              entry,
              "Terminal buffer unavailable. Resize the window to request a repaint."
            );
          }
          return;
        }

        if (
          snapshot.rows !== null &&
          snapshot.cols !== null &&
          (entry.terminal.rows !== snapshot.rows || entry.terminal.cols !== snapshot.cols)
        ) {
          resizeTerminalForSnapshot(entry, snapshot.cols, snapshot.rows);
        }

        writeTerminalBytes(entry, new Uint8Array(snapshot.data));

        if (sessionId === activeSessionId) {
          scheduleViewportSync(sessionId);
        }
      })
      .catch((err) => {
        console.warn("[terminal] snapshot replay failed:", err);
        if (terminals.get(sessionId) === entry && !entry.hasRenderedOutput) {
          setReplayStatus(
            entry,
            "Terminal buffer unavailable. Resize the window to request a repaint."
          );
        }
      });
  };

  const createSessionTerminal = (sessionId: string) => {
    const existing = terminals.get(sessionId);
    if (existing) {
      return existing;
    }

    const container = document.createElement("div");
    container.className = "terminal-instance";
    container.dataset.sessionId = sessionId;
    container.setAttribute("data-ac-testid", `terminal.session.${sessionId}`);
    container.setAttribute("data-ac-role", "surface");
    container.setAttribute("data-ac-session-id", sessionId);
    container.hidden = true;
    hostRef.appendChild(container);

    const terminal = new Terminal(createTerminalOptions(isTauri));

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);

    const replayStatus = document.createElement("div");
    replayStatus.className = "terminal-replay-status";
    replayStatus.hidden = true;
    replayStatus.setAttribute("data-ac-testid", `terminal.replay-status.${sessionId}`);
    container.appendChild(replayStatus);

    try {
      const webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        webglAddon.dispose();
      });
      terminal.loadAddon(webglAddon);
    } catch {
      // Canvas renderer fallback is automatic.
    }

    const entry: SessionTerminal = {
      container,
      terminal,
      fitAddon,
      inputBuffer: "",
      snapshotReplayRequested: false,
      snapshotResizeSuppressed: false,
      hasRenderedOutput: false,
      replayStatus,
    };

    // Per-terminal keyboard shortcuts. Match keys via event.key (layout-aware,
    // matches the convention in shared/shortcuts.ts) so Dvorak/Colemak/AZERTY
    // users press the key labeled C/V in their layout, not the QWERTY position.
    terminal.attachCustomKeyEventHandler((event) => {
      // IME composition: hand the keystroke back to xterm so its internal
      // _compositionHelper can finish the multi-keystroke sequence. Some CJK
      // IMEs chord Ctrl+Shift during composition; intercepting would corrupt
      // the IME state.
      if (event.isComposing) return true;

      const isCtrlShift = event.ctrlKey && event.shiftKey;
      const key = event.key.toLowerCase();

      // Ctrl+Shift+C → copy xterm selection to system clipboard. With no
      // selection, return true so the event falls through (in dev, WebView2
      // opens DevTools; in production, nothing happens).
      if (isCtrlShift && key === "c") {
        if (event.type === "keydown") {
          if (!terminal.hasSelection()) return true;
          event.preventDefault();
          event.stopPropagation();
          navigator.clipboard
            .writeText(terminal.getSelection())
            .catch((err) => console.warn("[copy] write failed:", err?.name ?? "Error"));
          return false;
        }
        // keyup mirror: re-check selection (stateless; selection persists post-copy).
        return terminal.hasSelection() ? false : true;
      }

      // Ctrl+Shift+V → paste system clipboard via terminal.paste() so the
      // payload is wrapped in bracketed-paste markers. clipboard.readText()
      // is async; re-check session and dispose state inside .then.
      if (isCtrlShift && key === "v") {
        if (event.type === "keydown") {
          event.preventDefault();
          event.stopPropagation();
          navigator.clipboard
            .readText()
            .then((text) => {
              if (!text) return;
              if (activeSessionId !== sessionId) return; // session switched during await
              if (terminal.element?.isConnected !== true) return; // terminal not in DOM (pre-open or post-dispose detached)
              // Strip both 7-bit ESC[200~/201~ and 8-bit C1 \x9b 200~/201~
              // forms — defense-in-depth against future shells that activate
              // 8-bit recognition. xterm@6.0.0 does NOT sanitize internally
              // (CVE-2019-11848 redux). See plan #104 §1.1.D / §6.8.
              const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, "");
              terminal.paste(sanitized);
            })
            .catch((err) => console.warn("[paste] read failed:", err?.name ?? "Error"));
          return false;
        }
        return false;
      }

      // Shift+Enter → send LF (soft newline) instead of CR (submit)
      if (event.key === "Enter" && event.shiftKey) {
        if (event.type === "keydown" && activeSessionId === sessionId) {
          const encoder = new TextEncoder();
          void PtyAPI.write(sessionId, encoder.encode("\n"));
        }
        return false; // suppress both keydown and keyup
      }
      return true;
    });

    terminal.onData((data) => {
      if (activeSessionId !== sessionId) {
        return;
      }

      const encoder = new TextEncoder();
      void PtyAPI.write(sessionId, encoder.encode(data));

      const capture = updatePromptCapture(entry.inputBuffer, data);
      entry.inputBuffer = capture.buffer;
      if (capture.submittedPrompt) {
        void SessionAPI.setLastPrompt(sessionId, capture.submittedPrompt);
      }
    });

    terminal.onResize(({ cols, rows }) => {
      if (activeSessionId !== sessionId || entry.snapshotResizeSuppressed) {
        return;
      }

      void PtyAPI.resize(sessionId, cols, rows);
    });

    terminals.set(sessionId, entry);
    replayNativeSnapshot(sessionId, entry);
    return entry;
  };

  const showSessionTerminal = (sessionId: string) => {
    const next = createSessionTerminal(sessionId);

    if (activeSessionId && activeSessionId !== sessionId) {
      const previous = terminals.get(activeSessionId);
      if (previous) {
        previous.container.hidden = true;
      }
    }

    next.container.hidden = false;
    activeSessionId = sessionId;
    next.terminal.focus();

    if (isBrowser) {
      // Browser mode: fit xterm to container and resize PTY to match.
      // The program receives SIGWINCH and redraws at the new size.
      // We skip the snapshot replay because it was rendered for the
      // native terminal's dimensions and would look garbled here.
      requestAnimationFrame(() => {
        if (sessionId !== activeSessionId) return;
        syncViewport(sessionId);
      });
    } else {
      next.terminal.scrollToBottom();
      scheduleViewportSync(sessionId);
    }
  };

  onMount(async () => {
    resizeObserver = new ResizeObserver(() => {
      if (activeSessionId) {
        scheduleViewportSync(activeSessionId);
      }
    });
    resizeObserver.observe(hostRef);

    unlistenPtyOutput = await onPtyOutput(({ sessionId, data }) => {
      const entry =
        terminals.get(sessionId) ?? (sessionId === activeSessionId
          ? createSessionTerminal(sessionId)
          : null);

      if (!entry) {
        return;
      }

      writeTerminalBytes(entry, new Uint8Array(data));
    });

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      disposeSessionTerminal(id);
    });

    // Plan §A2.3.G6 pre-warm: in main-window mode, subscribe to
    // terminal_detached events and pre-create hidden xterm entries so
    // pty_output for detached sessions accumulates in main's cache. On
    // re-attach, showSessionTerminal promotes the pre-warmed entry to
    // visible with full scrollback intact. Skip in detached windows.
    if (!props.lockedSessionId) {
      unlistenTerminalDetached = await onTerminalDetached(({ sessionId }) => {
        if (!terminals.has(sessionId)) {
          const entry = createSessionTerminal(sessionId);
          entry.container.hidden = true;
        }
      });
    }
  });

  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    if (!sessionId) {
      if (activeSessionId) {
        const activeEntry = terminals.get(activeSessionId);
        if (activeEntry) {
          activeEntry.container.hidden = true;
        }
      }
      activeSessionId = null;
      return;
    }

    showSessionTerminal(sessionId);
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    unlistenSessionDestroyed?.();
    unlistenTerminalDetached?.();
    resizeObserver?.disconnect();

    for (const sessionId of Array.from(terminals.keys())) {
      disposeSessionTerminal(sessionId);
    }
  });

  return (
    <div
      class="terminal-host"
      ref={hostRef!}
      data-ac-testid="terminal.host"
      data-ac-role="surface"
    >
      <textarea
        class="terminal-automation-input"
        aria-label="Terminal automation input"
        disabled={!terminalStore.activeSessionId}
        tabIndex={-1}
        data-ac-testid="terminal.input"
        data-ac-role="textbox"
        data-ac-state={terminalStore.activeSessionId ? "ready" : "disabled"}
        onInput={(event) => {
          const value = event.currentTarget.value;
          event.currentTarget.value = "";
          writeAutomationInput(value);
        }}
      />
    </div>
  );
};

export default TerminalView;
