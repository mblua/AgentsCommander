/// Transport abstraction over Tauri IPC or WebSocket.
/// Both implementations must satisfy this interface.
export interface Transport {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    callback: (payload: T) => void
  ): Promise<() => void>;
  /** Emit an event to all windows */
  emit<T>(event: string, payload: T): Promise<void>;
  /** Synchronous local connection snapshot; never sent through backend events. */
  connectionState(): TransportConnectionState;
  /** Optional local lifecycle subscription. */
  onConnectionState?(callback: (state: TransportConnectionState) => void): UnlistenFn;
  /** Efficient binary PTY write (optional — falls back to invoke if absent) */
  writePtyBinary?(sessionId: string, data: Uint8Array): void;
}

export type UnlistenFn = () => void;

export interface TransportConnectionState {
  state: "connected" | "disconnected";
  generation: number;
}
