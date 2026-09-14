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

function sentMessages(socket: MockWebSocket): { id: number; cmd: string }[] {
  return socket.sent.map(
    (raw) => JSON.parse(String(raw)) as { id: number; cmd: string },
  );
}

function respondWith(socket: MockWebSocket, id: number, result: unknown): void {
  socket.message(
    JSON.stringify({ event: "__cmd_response", payload: { data: { id, result } } }),
  );
}

function trackInvoke<T>(promise: Promise<T>): {
  promise: Promise<T>;
  state: () => "pending" | "resolved" | "rejected";
} {
  let state: "pending" | "resolved" | "rejected" = "pending";
  const tracked = promise.then(
    (value) => {
      state = "resolved";
      return value;
    },
    (error: unknown) => {
      state = "rejected";
      throw error;
    },
  );
  void tracked.catch(() => undefined);
  return { promise: tracked, state: () => state };
}

/** Let `invoke` finish `waitForConnection` and register its pending request. */
async function flushInvoke(): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
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

describe("WsTransport backend-owned completion (#1942)", () => {
  const BACKEND_OWNED_COMMANDS = [
    "apply_coding_agent_profile_selection",
    "apply_selection_lock_removal",
    "set_replica_selection_default",
    "set_instance_profile_override",
  ];
  const LOCALLY_TIMED_COMMANDS = [
    "preview_coding_agent_profile_selection",
    "preview_selection_lock_removal",
    "get_replica_selection_default",
  ];

  beforeEach(() => {
    vi.useFakeTimers();
    MockWebSocket.instances.length = 0;
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function openTransport(): { transport: WsTransport; socket: MockWebSocket } {
    const transport = new WsTransport();
    const socket = MockWebSocket.instances[0];
    socket.open();
    return { transport, socket };
  }

  it("still rejects an ordinary command at the unchanged 30s local timeout", async () => {
    const { transport, socket } = openTransport();
    const call = trackInvoke(transport.invoke("get_settings"));
    await flushInvoke();
    expect(sentMessages(socket)).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(29_999);
    expect(call.state()).toBe("pending");

    await vi.advanceTimersByTimeAsync(1);
    await expect(call.promise).rejects.toBe("Command timeout: get_settings");
    transport.close();
  });

  it.each(BACKEND_OWNED_COMMANDS)(
    "%s stays pending past 30s and resolves on its own matching response",
    async (cmd) => {
      const { transport, socket } = openTransport();
      const call = trackInvoke(
        transport.invoke(cmd, { request: { marker: cmd } }),
      );
      await flushInvoke();
      const [sent] = sentMessages(socket);
      expect(sent.cmd).toBe(cmd);

      await vi.advanceTimersByTimeAsync(120_000);
      expect(call.state()).toBe("pending");

      respondWith(socket, sent.id, { accepted: cmd });
      await expect(call.promise).resolves.toEqual({ accepted: cmd });
      expect(call.state()).toBe("resolved");
      transport.close();
    },
  );

  it("still rejects a pending backend-owned mutation on disconnect", async () => {
    const { transport, socket } = openTransport();
    const call = trackInvoke(
      transport.invoke("apply_selection_lock_removal", { request: {} }),
    );
    await flushInvoke();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(call.state()).toBe("pending");

    socket.close();
    await expect(call.promise).rejects.toBe("WebSocket closed");
    transport.close();
  });

  it("ignores the late response of a request that already timed out", async () => {
    const { transport, socket } = openTransport();
    const timedOut = trackInvoke(transport.invoke("get_settings"));
    await flushInvoke();
    const [first] = sentMessages(socket);

    await vi.advanceTimersByTimeAsync(30_000);
    await expect(timedOut.promise).rejects.toBe("Command timeout: get_settings");

    respondWith(socket, first.id, { stale: true });
    await flushInvoke();
    expect(timedOut.state()).toBe("rejected");

    const fresh = trackInvoke(transport.invoke("get_settings"));
    await flushInvoke();
    const freshSent = sentMessages(socket)[1];

    respondWith(socket, first.id, { stale: true, again: true });
    await flushInvoke();
    expect(fresh.state()).toBe("pending");

    respondWith(socket, freshSent.id, { fresh: true });
    await expect(fresh.promise).resolves.toEqual({ fresh: true });
    transport.close();
  });

  it.each(LOCALLY_TIMED_COMMANDS)(
    "%s times out without rejecting, replaying or replacing the pending mutation",
    async (readCmd) => {
      const { transport, socket } = openTransport();
      const mutation = trackInvoke(
        transport.invoke("apply_coding_agent_profile_selection", {
          request: { scope: "kind" },
        }),
      );
      const read = trackInvoke(
        transport.invoke(readCmd, { request: { scope: "kind" } }),
      );
      await flushInvoke();
      const [mutationSent, readSent] = sentMessages(socket);
      expect(mutationSent.cmd).toBe("apply_coding_agent_profile_selection");
      expect(readSent.cmd).toBe(readCmd);

      await vi.advanceTimersByTimeAsync(30_000);
      await expect(read.promise).rejects.toBe(`Command timeout: ${readCmd}`);
      expect(mutation.state()).toBe("pending");
      expect(sentMessages(socket)).toHaveLength(2);

      respondWith(socket, readSent.id, { late: "read" });
      await flushInvoke();
      expect(mutation.state()).toBe("pending");

      const fresh = trackInvoke(
        transport.invoke(readCmd, { request: { scope: "kind" } }),
      );
      await flushInvoke();
      const freshSent = sentMessages(socket)[2];

      respondWith(socket, readSent.id, { late: "read-again" });
      await flushInvoke();
      expect(fresh.state()).toBe("pending");
      expect(sentMessages(socket)).toHaveLength(3);

      respondWith(socket, freshSent.id, { fresh: true });
      await expect(fresh.promise).resolves.toEqual({ fresh: true });

      respondWith(socket, mutationSent.id, { done: true });
      await expect(mutation.promise).resolves.toEqual({ done: true });
      transport.close();
    },
  );

  it("keeps events and connection generation unaffected while a mutation is pending", async () => {
    const transport = new WsTransport();
    const states: unknown[] = [];
    transport.onConnectionState((state) => states.push(state));
    const payloads: unknown[] = [];
    await transport.listen("session_switched", (payload) => payloads.push(payload));

    const socket = MockWebSocket.instances[0];
    socket.open();
    const call = trackInvoke(
      transport.invoke("set_instance_profile_override", {
        agentPath: "C:\\replica",
        profile: null,
      }),
    );
    await flushInvoke();
    const [sent] = sentMessages(socket);

    await vi.advanceTimersByTimeAsync(60_000);
    expect(call.state()).toBe("pending");
    expect(transport.connectionState()).toEqual({ state: "connected", generation: 1 });
    expect(states).toEqual([{ state: "connected", generation: 1 }]);

    socket.message(
      JSON.stringify({ event: "session_switched", payload: { id: "s-1" } }),
    );
    expect(payloads).toEqual([{ id: "s-1" }]);

    respondWith(socket, sent.id, { ok: true });
    await expect(call.promise).resolves.toEqual({ ok: true });
    transport.close();
  });
});
