import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  onPtyOutput,
  onSessionDestroyed,
} from "../../shared/ipc";
import { terminalStore } from "../stores/terminal";
import type { UnlistenFn } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";

interface SessionTerminal {
  container: HTMLDivElement;
  terminal: Terminal;
  fitAddon: FitAddon;
  inputBuffer: string;
}

const TerminalView: Component = () => {
  let hostRef!: HTMLDivElement;
  let activeSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;

  const terminals = new Map<string, SessionTerminal>();

  const syncViewport = (sessionId: string) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    terminalStore.setTermSize(entry.terminal.cols, entry.terminal.rows);
    void PtyAPI.resize(sessionId, entry.terminal.cols, entry.terminal.rows);
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

  const createSessionTerminal = (sessionId: string) => {
    const existing = terminals.get(sessionId);
    if (existing) {
      return existing;
    }

    const container = document.createElement("div");
    container.className = "terminal-instance";
    container.dataset.sessionId = sessionId;
    container.hidden = true;
    hostRef.appendChild(container);

    const terminal = new Terminal({
      fontFamily: "'Cascadia Code', 'JetBrains Mono', 'Fira Code', monospace",
      fontSize: 14,
      lineHeight: 1.2,
      cursorBlink: true,
      cursorStyle: "block",
      scrollback: 10000,
      theme: {
        background: "#0a0a0f",
        foreground: "#e8e8e8",
        cursor: "#00d4ff",
        selectionBackground: "rgba(0, 212, 255, 0.25)",
        black: "#1a1a2e",
        red: "#ff3b5c",
        green: "#33ff99",
        yellow: "#ffcc33",
        blue: "#3399ff",
        magenta: "#ff33cc",
        cyan: "#33ccff",
        white: "#e8e8e8",
        brightBlack: "#4a4a5e",
        brightRed: "#ff6699",
        brightGreen: "#66ffbb",
        brightYellow: "#ffdd66",
        brightBlue: "#66bbff",
        brightMagenta: "#ff66dd",
        brightCyan: "#66ddff",
        brightWhite: "#ffffff",
      },
      allowTransparency: false,
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);

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
    };

    terminal.onData((data) => {
      if (activeSessionId !== sessionId) {
        return;
      }

      const encoder = new TextEncoder();
      void PtyAPI.write(sessionId, encoder.encode(data));

      if (data === "\r") {
        const trimmed = entry.inputBuffer.trim();
        if (trimmed) {
          void SessionAPI.setLastPrompt(sessionId, trimmed);
        }
        entry.inputBuffer = "";
      } else if (data === "\x7f") {
        entry.inputBuffer = entry.inputBuffer.slice(0, -1);
      } else if (data.length === 1 && data >= " ") {
        entry.inputBuffer += data;
      } else if (data.length > 1 && !data.startsWith("\x1b")) {
        entry.inputBuffer += data;
      }
    });

    terminal.onResize(({ cols, rows }) => {
      if (activeSessionId !== sessionId) {
        return;
      }

      terminalStore.setTermSize(cols, rows);
      void PtyAPI.resize(sessionId, cols, rows);
    });

    terminals.set(sessionId, entry);
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
    next.terminal.scrollToBottom();
    scheduleViewportSync(sessionId);
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

      entry.terminal.write(new Uint8Array(data), () => {
        if (sessionId === activeSessionId) {
          entry.terminal.scrollToBottom();
        }
      });
    });

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      disposeSessionTerminal(id);
    });
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
      terminalStore.setTermSize(0, 0);
      return;
    }

    showSessionTerminal(sessionId);
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    unlistenSessionDestroyed?.();
    resizeObserver?.disconnect();

    for (const sessionId of Array.from(terminals.keys())) {
      disposeSessionTerminal(sessionId);
    }
  });

  return <div class="terminal-host" ref={hostRef!} />;
};

export default TerminalView;
