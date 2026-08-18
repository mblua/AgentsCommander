import type {
  ListenOptions,
  Transport,
  TransportConnectionState,
  UnlistenFn,
} from "./transport";

/// Transport implementation using Tauri's native IPC.
/// Uses dynamic imports to avoid failing in non-Tauri environments.
export class TauriTransport implements Transport {
  private invokeImpl: ((cmd: string, args?: Record<string, unknown>) => Promise<unknown>) | null = null;
  private listenImpl:
    | ((
        event: string,
        handler: (e: { payload: unknown }) => void,
        options?: ListenOptions
      ) => Promise<() => void>)
    | null = null;
  private emitImpl: ((event: string, payload?: unknown) => Promise<void>) | null = null;
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
    // A string `target` reaches Tauri as `{ kind: 'AnyLabel', label }`
    // (`@tauri-apps/api/event.js:69-73`); omitting it registers `Any`, which
    // no `emit_to` filter can bind. See `ListenOptions`.
    return this.listenImpl!(event, (e) => callback(e.payload as T), options);
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
