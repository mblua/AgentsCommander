/**
 * #1363 - per-listener event scoping.
 *
 * Tauri's JS default registers a listener as `EventTarget::Any`, and the Rust
 * side SHORT-CIRCUITS the label filter for such a listener
 * (`tauri-2.10.3/src/event/listener.rs:306-311`, `match_any_or_filter`), so an
 * `Any` listener receives every `emit_to` regardless of its label. Setting
 * `scopeToCurrentWindow` registers `AnyLabel` for the window the listener runs
 * in, which the backend's `emit_to(label, ...)` filter matches, and which is
 * what makes backend-side scoping actually bind.
 *
 * It is a FLAG rather than a label, so a caller cannot pass the wrong window's
 * label: only the transport knows which webview it runs in, exactly as only
 * Tauri knows which webview called a command.
 *
 * Leaving it unset is still correct for every event the backend delivers with
 * a plain `emit`: that path passes no filter, and with no filter every handler
 * matches whatever its target is (`listener.rs:296-302`).
 */
export interface ListenOptions {
  scopeToCurrentWindow?: boolean;
}

/// Transport abstraction over Tauri IPC or WebSocket.
/// Both implementations must satisfy this interface.
export interface Transport {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    callback: (payload: T) => void,
    options?: ListenOptions
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
