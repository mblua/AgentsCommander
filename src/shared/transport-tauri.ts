import type {
  ListenOptions,
  Transport,
  TransportConnectionState,
  UnlistenFn,
} from "./transport";

/** The subset of `@tauri-apps/api/event`'s listen options this transport uses.
 *  A string `target` reaches Tauri as `{ kind: 'AnyLabel', label }`
 *  (`@tauri-apps/api/event.js:69-73`). */
interface TauriListenOptions {
  target?: string;
}

/// Transport implementation using Tauri's native IPC.
/// Uses dynamic imports to avoid failing in non-Tauri environments.
export class TauriTransport implements Transport {
  private invokeImpl: ((cmd: string, args?: Record<string, unknown>) => Promise<unknown>) | null = null;
  private listenImpl:
    | ((
        event: string,
        handler: (e: { payload: unknown }) => void,
        options?: TauriListenOptions
      ) => Promise<() => void>)
    | null = null;
  private emitImpl: ((event: string, payload?: unknown) => Promise<void>) | null = null;
  /**
   * #1363 - this webview's label, resolved HERE rather than at the call site.
   *
   * It lives in `init()` because `init()` is already awaited by every method
   * through `this.ready`: resolving it lazily inside `listen` would put a
   * module load between a caller asking to listen and the listener actually
   * registering, and a `pty_output` emitted in that window is lost outright
   * (there is no replay). Resolving it here also keeps the dynamic import that
   * lets the bundler split `@tauri-apps/api` out of the main chunk.
   */
  private currentWindowLabel: string | null = null;
  private ready: Promise<void>;

  constructor() {
    this.ready = this.init();
  }

  private async init() {
    const core = await import("@tauri-apps/api/core");
    const event = await import("@tauri-apps/api/event");
    this.invokeImpl = core.invoke;
    this.listenImpl = event.listen as typeof this.listenImpl;
    this.emitImpl = event.emit;

    // Never allowed to reject: `this.ready` is awaited by invoke/listen/emit
    // alike, so a throw here would take the whole transport down. An
    // unresolved label degrades a scoped listener to the unscoped `Any`
    // registration the app shipped before #1363 — a bandwidth cost, not a
    // black terminal.
    try {
      const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      this.currentWindowLabel = getCurrentWebviewWindow().label;
    } catch (error) {
      console.warn("[transport] current webview label unavailable:", error);
    }
  }

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    await this.ready;
    return this.invokeImpl!(cmd, args) as Promise<T>;
  }

  async listen<T>(
    event: string,
    callback: (payload: T) => void,
    options?: ListenOptions
  ): Promise<() => void> {
    await this.ready;
    const scoped =
      options?.scopeToCurrentWindow === true && this.currentWindowLabel !== null
        ? { target: this.currentWindowLabel }
        : undefined;
    return this.listenImpl!(event, (e) => callback(e.payload as T), scoped);
  }

  async emit<T>(event: string, payload: T): Promise<void> {
    await this.ready;
    await this.emitImpl!(event, payload);
  }

  connectionState(): TransportConnectionState {
    return { state: "connected", generation: 0 };
  }

  onConnectionState(_callback: (state: TransportConnectionState) => void): UnlistenFn {
    return () => undefined;
  }
}
