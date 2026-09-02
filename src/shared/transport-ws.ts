import type {
  ListenOptions,
  Transport,
  TransportConnectionState,
  UnlistenFn,
} from "./transport";
import type { PtyOutputEvent } from "./types";
import { noteInvokeSettle, noteInvokeStart } from "./ipc-blackbox";

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason: string) => void;
}

/// Transport implementation using WebSocket.
/// Connects to the embedded axum server for remote browser access.
export class WsTransport implements Transport {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, PendingRequest>();
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  private reconnectDelay = 1000;
  private maxReconnectDelay = 10000;
  private connected = false;
  private generation = 0;
  private connectionListeners = new Set<(state: TransportConnectionState) => void>();
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;
  private url: string;

  constructor() {
    const params = new URLSearchParams(window.location.search);
    const token = params.get("remoteToken") || sessionStorage.getItem("remoteToken") || "";
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    this.url = `${proto}//${location.host}/ws?token=${token}`;
    this.connect();
  }

  private connect() {
    if (this.closed) return;

    try {
      const socket = new WebSocket(this.url);
      this.ws = socket;
      socket.binaryType = "arraybuffer";

      socket.onopen = () => {
        if (this.ws !== socket || this.closed) {
          socket.close();
          return;
        }
        if (this.connected) return;
        this.connected = true;
        this.generation += 1;
        this.reconnectDelay = 1000;
        console.debug("[ws-transport] Connected");
        this.publishConnectionState();
      };

      socket.onmessage = (event) => {
        if (this.ws !== socket || this.closed) return;
        if (event.data instanceof ArrayBuffer) {
          this.handleBinary(new Uint8Array(event.data));
        } else {
          this.handleText(event.data as string);
        }
      };

      socket.onclose = () => {
        if (this.ws !== socket) return;
        this.ws = null;
        const wasConnected = this.connected;
        this.connected = false;
        this.rejectAllPending("WebSocket closed");
        if (wasConnected) this.publishConnectionState();
        this.scheduleReconnect();
      };

      socket.onerror = () => {
        // onclose will fire after this
      };
    } catch {
      this.scheduleReconnect();
    }
  }

  private scheduleReconnect() {
    if (this.closed || this.reconnectTimer) return;

    const delay =
      this.reconnectDelay +
      Math.floor(Math.random() * Math.min(250, this.reconnectDelay));
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (this.closed) return;
      if (
        this.ws &&
        (this.ws.readyState === WebSocket.OPEN ||
          this.ws.readyState === WebSocket.CONNECTING)
      ) {
        return;
      }
      console.debug(
        `[ws-transport] Reconnecting after ${delay}ms...`
      );
      this.connect();
      this.reconnectDelay = Math.min(
        this.reconnectDelay * 2,
        this.maxReconnectDelay
      );
    }, delay);
  }

  close() {
    this.closed = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    const socket = this.ws;
    this.ws = null;
    this.connected = false;
    if (
      socket &&
      (socket.readyState === WebSocket.OPEN ||
        socket.readyState === WebSocket.CONNECTING)
    ) {
      socket.close();
    }
    this.rejectAllPending("WebSocket closed");
    this.connectionListeners.clear();
  }

  connectionState(): TransportConnectionState {
    return {
      state: this.connected ? "connected" : "disconnected",
      generation: this.generation,
    };
  }

  onConnectionState(callback: (state: TransportConnectionState) => void): UnlistenFn {
    this.connectionListeners.add(callback);
    return () => this.connectionListeners.delete(callback);
  }

  private publishConnectionState(): void {
    const snapshot = this.connectionState();
    for (const callback of Array.from(this.connectionListeners)) {
      try {
        callback(snapshot);
      } catch (error) {
        console.error("[ws-transport] Connection listener error:", error);
      }
    }
  }

  private rejectAllPending(reason: string) {
    for (const [, req] of this.pending) {
      req.reject(reason);
    }
    this.pending.clear();
  }

  private handleText(text: string) {
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(text);
    } catch {
      return;
    }

    // Command response routed via __cmd_response event
    if (parsed.event === "__cmd_response") {
      const payload = parsed.payload as Record<string, unknown>;
      const data = payload.data as Record<string, unknown>;
      const id = data.id as number;
      const pending = this.pending.get(id);
      if (pending) {
        this.pending.delete(id);
        if ("error" in data && data.error) {
          pending.reject(data.error as string);
        } else {
          pending.resolve(data.result);
        }
      }
      return;
    }

    // Regular event
    if (parsed.event && parsed.payload !== undefined) {
      const eventName = parsed.event as string;
      const callbacks = this.listeners.get(eventName);
      if (callbacks) {
        for (const cb of callbacks) {
          try {
            cb(parsed.payload);
          } catch (e) {
            console.error(`[ws-transport] Listener error for ${eventName}:`, e);
          }
        }
      }
    }
  }

  private handleBinary(data: Uint8Array) {
    if (data.length < 36) return;

    // First 36 bytes = session UUID ASCII
    const sessionId = new TextDecoder().decode(data.slice(0, 36)).trim();
    const ptyData = data.slice(36);

    // Convert to number[] to match PtyOutputEvent.data contract
    const dataArray = Array.from(ptyData);

    const callbacks = this.listeners.get("pty_output");
    if (callbacks) {
      // #1363: the payload is the same flat shape the Tauri window receives,
      // minus `sequence` (this transport carries no parser sequence). An
      // unsequenced event is written live with no reconcile, so both
      // transports traverse the one writer in TerminalView.
      const payload: PtyOutputEvent = { sessionId, data: dataArray };
      for (const cb of callbacks) {
        try {
          cb(payload);
        } catch (e) {
          console.error("[ws-transport] PTY output listener error:", e);
        }
      }
    }
  }

  /** Wait for WebSocket to be connected, with timeout. */
  private waitForConnection(timeoutMs = 5000): Promise<void> {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      return Promise.resolve();
    }
    return new Promise((resolve, reject) => {
      const interval = setInterval(() => {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
          clearInterval(interval);
          clearTimeout(timeout);
          resolve();
        }
      }, 50);
      const timeout = setTimeout(() => {
        clearInterval(interval);
        reject(new Error("WebSocket connection timeout"));
      }, timeoutMs);
    });
  }

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    const id = noteInvokeStart(cmd);
    try { return await this.invokeInner<T>(cmd, args); } finally { noteInvokeSettle(id); }
  }

  private async invokeInner<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    await this.waitForConnection();

    const id = this.nextId++;
    const msg = JSON.stringify({ id, cmd, args: args || {} });

    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
      });
      this.ws!.send(msg);

      // Timeout after 30s
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(`Command timeout: ${cmd}`);
        }
      }, 30000);
    });
  }

  async listen<T>(
    event: string,
    callback: (payload: T) => void,
    // Window labels are a Tauri concept: this transport has one client per
    // socket and no fan-out to scope, so the option is accepted and ignored.
    _options?: ListenOptions
  ): Promise<UnlistenFn> {
    let callbacks = this.listeners.get(event);
    if (!callbacks) {
      callbacks = new Set();
      this.listeners.set(event, callbacks);
    }
    const cb = callback as (payload: unknown) => void;
    callbacks.add(cb);

    return () => {
      const set = this.listeners.get(event);
      if (set) {
        set.delete(cb);
        if (set.size === 0) {
          this.listeners.delete(event);
        }
      }
    };
  }

  async emit<T>(event: string, payload: T): Promise<void> {
    await this.invoke<void>("broadcast_event", { event, payload });
  }

  writePtyBinary(sessionId: string, data: Uint8Array): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    // Binary frame: 36-byte UUID ASCII + raw bytes
    const idBytes = new TextEncoder().encode(sessionId.padEnd(36));
    const frame = new Uint8Array(36 + data.length);
    frame.set(idBytes.slice(0, 36), 0);
    frame.set(data, 36);
    this.ws.send(frame.buffer);
  }
}
