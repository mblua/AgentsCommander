import { Component, createEffect, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import {
  PtyAPI,
  SessionAPI,
  onPtyOutput,
  onPtyResized,
  onSessionDestroyed,
} from "../../shared/ipc";
import { isBrowser } from "../../shared/platform";
import { terminalStore } from "../stores/terminal";
import type { UnlistenFn } from "../../shared/transport";
import "@xterm/xterm/css/xterm.css";

interface SessionTerminal {
  container: HTMLDivElement;
  terminal: Terminal;
  fitAddon: FitAddon;
  inputBuffer: string;
  ptyRows?: number;
  ptyCols?: number;
}

const TerminalView: Component = () => {
  let hostRef!: HTMLDivElement;
  let activeSessionId: string | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let unlistenPtyResized: UnlistenFn | null = null;
  let unlistenSessionDestroyed: UnlistenFn | null = null;

  const terminals = new Map<string, SessionTerminal>();

  const syncViewport = (sessionId: string, skipPtyResize = false) => {
    const entry = terminals.get(sessionId);
    if (!entry) {
      return;
    }

    entry.fitAddon.fit();
    terminalStore.setTermSize(entry.terminal.cols, entry.terminal.rows);
    if (!skipPtyResize) {
      void PtyAPI.resize(sessionId, entry.terminal.cols, entry.terminal.rows);
    }
  };

  const scheduleViewportSync = (sessionId: string) => {
    if (isBrowser) {
      // Browser mode: recalculate font size for locked PTY dimensions
      requestAnimationFrame(() => {
        if (sessionId !== activeSessionId) return;
        const entry = terminals.get(sessionId);
        if (entry?.ptyRows && entry?.ptyCols) {
          lockToPtyDimensions(entry, entry.ptyRows, entry.ptyCols);
        }
      });
      return;
    }

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

    // Shift+Enter → send LF (soft newline) instead of CR (submit)
    terminal.attachCustomKeyEventHandler((event) => {
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
      // Browser is a read-only mirror — never resize the actual PTY
      if (!isBrowser) {
        void PtyAPI.resize(sessionId, cols, rows);
      }
    });

    terminals.set(sessionId, entry);
    return entry;
  };

  /**
   * Lock the browser xterm.js to exact PTY dimensions, scaling font size
   * so the terminal fills the container. This makes the browser a true
   * 1:1 mirror — all absolute cursor positioning works correctly.
   */
  const lockToPtyDimensions = (entry: SessionTerminal, ptyRows: number, ptyCols: number) => {
    const rect = entry.container.getBoundingClientRect();
    if (rect.height === 0 || rect.width === 0) return;

    // Use fitAddon to measure how many cells fit at the current font size
    const dims = entry.fitAddon.proposeDimensions();
    if (!dims || dims.cols === 0 || dims.rows === 0) return;

    const currentFontSize = entry.terminal.options.fontSize || 14;

    // Scale font so that ptyCols/ptyRows fill the container
    const scaleCols = dims.cols / ptyCols;
    const scaleRows = dims.rows / ptyRows;
    const scale = Math.min(scaleCols, scaleRows);

    const newFontSize = Math.max(Math.floor(currentFontSize * scale), 6);
    entry.terminal.options.fontSize = newFontSize;
    entry.terminal.resize(ptyCols, ptyRows);

    entry.ptyRows = ptyRows;
    entry.ptyCols = ptyCols;
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
      // Browser mode: dimension-locked mirror. Get PTY size and lock
      // xterm dimensions BEFORE subscribing, so the snapshot renders
      // at the correct size (the broadcast arrives before the response).
      requestAnimationFrame(() => {
        if (sessionId !== activeSessionId) return;
        PtyAPI.getPtySize(sessionId).then((size) => {
          if (sessionId !== activeSessionId || !size) return;
          lockToPtyDimensions(next, size.rows, size.cols);
          PtyAPI.subscribe(sessionId);
        });
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

      entry.terminal.write(new Uint8Array(data), () => {
        // Browser mode: dimension-locked mirror has no scrollback to manage.
        // scrollToBottom would push content off-screen if called before resize.
        if (sessionId === activeSessionId && !isBrowser) {
          entry.terminal.scrollToBottom();
        }
      });
    });

    unlistenSessionDestroyed = await onSessionDestroyed(({ id }) => {
      disposeSessionTerminal(id);
    });

    if (isBrowser) {
      unlistenPtyResized = await onPtyResized(({ sessionId, rows, cols }) => {
        const entry = terminals.get(sessionId);
        if (entry) {
          lockToPtyDimensions(entry, rows, cols);
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
      terminalStore.setTermSize(0, 0);
      return;
    }

    showSessionTerminal(sessionId);
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    unlistenPtyResized?.();
    unlistenSessionDestroyed?.();
    resizeObserver?.disconnect();

    for (const sessionId of Array.from(terminals.keys())) {
      disposeSessionTerminal(sessionId);
    }
  });

  return <div class="terminal-host" ref={hostRef!} />;
};

export default TerminalView;
