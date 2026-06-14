import type { Transport, UnlistenFn } from "../transport";

export interface InvokeCall {
  cmd: string;
  args: Record<string, unknown>;
}

type InvokeHandler = (
  args: Record<string, unknown>
) => unknown | Promise<unknown>;

interface FakeTransportOptions {
  binaryPty?: boolean;
}

export class FakeTransport implements Transport {
  readonly calls: InvokeCall[] = [];
  readonly binaryWrites: { sessionId: string; data: number[] }[] = [];
  writePtyBinary?: (sessionId: string, data: Uint8Array) => void;

  private readonly handlers = new Map<string, InvokeHandler>();
  private readonly listeners = new Map<string, Set<(payload: unknown) => void>>();

  constructor(opts?: FakeTransportOptions) {
    if (opts?.binaryPty) {
      this.writePtyBinary = (sessionId, data) => {
        this.binaryWrites.push({ sessionId, data: Array.from(data) });
      };
    }
  }

  onInvoke(cmd: string, handler: InvokeHandler): void {
    this.handlers.set(cmd, handler);
  }

  resolve(cmd: string, value: unknown): void {
    this.onInvoke(cmd, () => value);
  }

  reject(cmd: string, reason: string): void {
    this.onInvoke(cmd, () => {
      throw reason;
    });
  }

  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    const normalizedArgs = args ?? {};
    this.calls.push({ cmd, args: normalizedArgs });

    const handler = this.handlers.get(cmd);
    if (!handler) {
      throw new Error(`Unhandled fake transport invoke: ${cmd}`);
    }

    return (await handler(normalizedArgs)) as T;
  }

  async listen<T>(
    event: string,
    callback: (payload: T) => void
  ): Promise<UnlistenFn> {
    let callbacks = this.listeners.get(event);
    if (!callbacks) {
      callbacks = new Set();
      this.listeners.set(event, callbacks);
    }

    const wrapped = callback as (payload: unknown) => void;
    callbacks.add(wrapped);

    return () => {
      const set = this.listeners.get(event);
      if (!set) return;
      set.delete(wrapped);
      if (set.size === 0) {
        this.listeners.delete(event);
      }
    };
  }

  async emit<T>(event: string, payload: T): Promise<void> {
    this.calls.push({
      cmd: "broadcast_event",
      args: { event, payload },
    });
  }

  emitFromBackend<T>(event: string, payload: T): void {
    const callbacks = this.listeners.get(event);
    if (!callbacks) return;

    for (const callback of Array.from(callbacks)) {
      callback(payload);
    }
  }

  callsFor(cmd: string): InvokeCall[] {
    return this.calls.filter((call) => call.cmd === cmd);
  }

  lastCall(cmd: string): InvokeCall | undefined {
    const calls = this.callsFor(cmd);
    return calls[calls.length - 1];
  }

  clearCalls(): void {
    this.calls.length = 0;
  }
}
