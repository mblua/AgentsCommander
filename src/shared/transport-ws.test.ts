// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WsTransport } from "./transport-ws";

class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static readonly instances: MockWebSocket[] = [];

  readonly url: string;
  readyState = MockWebSocket.CONNECTING;
  binaryType = "blob";
  readonly sent: unknown[] = [];
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;

  constructor(url: string | URL) {
    this.url = String(url);
    MockWebSocket.instances.push(this);
  }

  open(): void {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event("open"));
  }

  message(data: string | ArrayBuffer): void {
    this.onmessage?.(new MessageEvent("message", { data }));
  }

  close(): void {
    if (this.readyState === MockWebSocket.CLOSED) return;
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }

  send(data: unknown): void {
    this.sent.push(data);
  }
}

describe("WsTransport connection generations", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances.length = 0;
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("snapshots a pre-subscription open and increments once per accepted reconnect", () => {
    const transport = new WsTransport();
    const first = MockWebSocket.instances[0];
    first.open();
    first.open();
    expect(transport.connectionState()).toEqual({ state: "connected", generation: 1 });

    const seen: unknown[] = [];
    const unlisten = transport.onConnectionState((state) => seen.push(state));
    first.close();
    expect(seen).toEqual([{ state: "disconnected", generation: 1 }]);

    vi.advanceTimersByTime(2_000);
    const second = MockWebSocket.instances[1];
    second.open();
    expect(seen).toEqual([
      { state: "disconnected", generation: 1 },
      { state: "connected", generation: 2 },
    ]);
    expect(transport.connectionState()).toEqual({ state: "connected", generation: 2 });
    expect(first.sent).toHaveLength(0);
    expect(second.sent).toHaveLength(0);

    unlisten();
    transport.close();
    expect(seen).toHaveLength(2);
  });

  it("keeps event subscriptions across reconnect and ignores superseded socket callbacks", async () => {
    const transport = new WsTransport();
    const payloads: unknown[] = [];
    await transport.listen("session_switched", (payload) => payloads.push(payload));

    const first = MockWebSocket.instances[0];
    first.open();
    first.close();
    vi.advanceTimersByTime(2_000);
    const second = MockWebSocket.instances[1];
    second.open();

    first.message(JSON.stringify({ event: "session_switched", payload: { stale: true } }));
    second.message(JSON.stringify({ event: "session_switched", payload: { fresh: true } }));
    expect(payloads).toEqual([{ fresh: true }]);
    transport.close();
  });

  it("reports one disconnected notification for an accepted close", () => {
    const transport = new WsTransport();
    const states: unknown[] = [];
    transport.onConnectionState((state) => states.push(state));
    const socket = MockWebSocket.instances[0];
    socket.open();
    socket.close();
    socket.close();
    expect(states).toEqual([
      { state: "connected", generation: 1 },
      { state: "disconnected", generation: 1 },
    ]);
    transport.close();
  });

  it("explicit close stops lifecycle notifications and reconnect attempts", () => {
    const transport = new WsTransport();
    const states: unknown[] = [];
    transport.onConnectionState((state) => states.push(state));
    const socket = MockWebSocket.instances[0];
    socket.open();
    transport.close();
    socket.open();
    socket.close();
    vi.advanceTimersByTime(20_000);
    expect(states).toEqual([{ state: "connected", generation: 1 }]);
    expect(MockWebSocket.instances).toHaveLength(1);
  });
});
