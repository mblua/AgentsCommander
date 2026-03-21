import { Component, onMount, onCleanup, createEffect } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { WebglAddon } from "@xterm/addon-webgl";
import { FitAddon } from "@xterm/addon-fit";
import { PtyAPI, SessionAPI, onPtyOutput } from "../../shared/ipc";
import { terminalStore } from "../stores/terminal";
import type { UnlistenFn } from "@tauri-apps/api/event";
import "@xterm/xterm/css/xterm.css";

const TerminalView: Component = () => {
  let containerRef!: HTMLDivElement;
  let terminal: Terminal | null = null;
  let fitAddon: FitAddon | null = null;
  let unlistenPtyOutput: UnlistenFn | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let inputBuffer = "";

  const initTerminal = () => {
    terminal = new Terminal({
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

    fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);

    terminal.open(containerRef);

    // Try WebGL addon, fall back silently
    try {
      const webglAddon = new WebglAddon();
      webglAddon.onContextLoss(() => {
        webglAddon.dispose();
      });
      terminal.loadAddon(webglAddon);
    } catch {
      // Canvas renderer fallback — automatic
    }

    fitAddon.fit();
    updateSize();

    // Handle user input → PTY write + track last prompt
    terminal.onData((data) => {
      const sessionId = terminalStore.activeSessionId;
      if (sessionId) {
        const encoder = new TextEncoder();
        PtyAPI.write(sessionId, encoder.encode(data));
      }

      // Track input for last-prompt display
      if (data === "\r") {
        const trimmed = inputBuffer.trim();
        if (trimmed && sessionId) {
          SessionAPI.setLastPrompt(sessionId, trimmed);
        }
        inputBuffer = "";
      } else if (data === "\x7f") {
        // Backspace
        inputBuffer = inputBuffer.slice(0, -1);
      } else if (data.length === 1 && data >= " ") {
        inputBuffer += data;
      } else if (data.length > 1 && !data.startsWith("\x1b")) {
        // Pasted text
        inputBuffer += data;
      }
    });

    // Handle resize
    terminal.onResize(({ cols, rows }) => {
      const sessionId = terminalStore.activeSessionId;
      if (sessionId) {
        PtyAPI.resize(sessionId, cols, rows);
      }
      terminalStore.setTermSize(cols, rows);
    });

    // Observe container resize
    resizeObserver = new ResizeObserver(() => {
      if (fitAddon) {
        fitAddon.fit();
      }
    });
    resizeObserver.observe(containerRef);
  };

  const updateSize = () => {
    if (terminal) {
      terminalStore.setTermSize(terminal.cols, terminal.rows);
    }
  };

  onMount(async () => {
    initTerminal();

    // Listen for PTY output
    unlistenPtyOutput = await onPtyOutput(({ sessionId, data }) => {
      if (sessionId === terminalStore.activeSessionId && terminal) {
        terminal.write(new Uint8Array(data));
      }
    });
  });

  // When the active session changes, clear terminal and resize PTY
  createEffect(() => {
    const sessionId = terminalStore.activeSessionId;
    if (sessionId && terminal && fitAddon) {
      terminal.clear();
      fitAddon.fit();
      // Send resize to new session PTY
      PtyAPI.resize(sessionId, terminal.cols, terminal.rows);
    }
  });

  onCleanup(() => {
    unlistenPtyOutput?.();
    resizeObserver?.disconnect();
    terminal?.dispose();
  });

  return <div class="terminal-container" ref={containerRef!} />;
};

export default TerminalView;
