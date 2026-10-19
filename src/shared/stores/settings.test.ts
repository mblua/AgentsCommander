import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../testing/fake-transport";
import { __setTransportForTests } from "../ipc";

const setSoundsEnabled = vi.hoisted(() => vi.fn());
vi.mock("../sound", () => ({ setSoundsEnabled }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(yes => { resolve = yes; });
  return { promise, resolve };
}

// Drain only promise continuations: these tests need no transport timers.
async function settle() { for (let i = 0; i < 20; i++) await Promise.resolve(); }

const value = (seconds: number, soundsEnabled = true) => ({ roomIdleConfirmationSeconds: seconds, soundsEnabled });

describe("settingsStore realm recovery", () => {
  let restore: (() => void) | undefined;
  let store: typeof import("./settings");
  let fake: FakeTransport;

  async function start() {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    store = await import("./settings");
    store.__disposeSettingsSubscriptionsForTests();
    setSoundsEnabled.mockClear();
    fake.resolve("get_settings", value(15));
  }

  afterEach(() => {
    store?.__disposeSettingsSubscriptionsForTests();
    restore?.();
    restore = undefined;
    vi.restoreAllMocks();
  });

  it("awaits all registrations before bootstrap and shares concurrent setup", async () => {
    await start();
    const gate = deferred<() => void>();
    const listen = fake.listen.bind(fake);
    vi.spyOn(fake, "listen").mockImplementation(async (event, callback, options) => {
      const off = await listen(event, callback, options);
      if (event === "room_idle_confirmation_updated") {
        fake.emitFromBackend(event, {});
        await gate.promise;
      }
      return off;
    });
    const connection = vi.spyOn(fake, "onConnectionState");
    const loads = [store.settingsStore.load(), store.settingsStore.load(), store.__subscribeCodingAgentSettingsUpdates()];
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(0);
    gate.resolve(() => undefined);
    await Promise.all(loads);
    expect(fake.callsFor("get_settings")).toHaveLength(1);
    expect(fake.listensFor("coding_agent_settings_updated")).toHaveLength(1);
    expect(fake.listensFor("room_idle_confirmation_updated")).toHaveLength(1);
    expect(connection).toHaveBeenCalledTimes(1);
    expect(store.settingsStore.current).toMatchObject(value(15));
  });

  it("retains coding-agent refresh and installs authoritative move order", async () => {
    await start();
    await store.__subscribeCodingAgentSettingsUpdates();
    fake.resolve("get_settings", { ...value(30), agents: [{ id: "claude" }, { id: "codex" }] });
    fake.emitFromBackend("coding_agent_settings_updated", { op: "move", agentId: "claude" });
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    expect(store.settingsStore.current?.agents.map(agent => agent.id)).toEqual(["claude", "codex"]);
  });

  it("reconnects without an event, ignoring duplicates and lower generations", async () => {
    await start();
    await store.settingsStore.load();
    fake.setConnectionState({ state: "disconnected", generation: 0 });
    fake.resolve("get_settings", value(40));
    fake.emitFromBackend("room_idle_confirmation_updated", {});
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(1);
    expect(store.settingsStore.current).toMatchObject(value(15));
    fake.setConnectionState({ state: "connected", generation: 1 });
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    expect(store.settingsStore.current).toMatchObject(value(40));
    fake.setConnectionState({ state: "connected", generation: 1 });
    fake.setConnectionState({ state: "disconnected", generation: 0 });
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    expect(store.settingsStore.current).toMatchObject(value(40));
  });

  it("starts disconnected and reconciles queued connection callbacks before bootstrap", async () => {
    await start();
    fake.setConnectionState({ state: "disconnected", generation: 2 });
    await store.settingsStore.load();
    expect(fake.callsFor("get_settings")).toHaveLength(0);
    expect(store.settingsStore.current).toBeNull();
    fake.setConnectionState({ state: "connected", generation: 3 });
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(1);
    expect(store.settingsStore.current).toMatchObject(value(15));
    store.__disposeSettingsSubscriptionsForTests();
    const subscribe = fake.onConnectionState.bind(fake);
    vi.spyOn(fake, "onConnectionState").mockImplementation(callback => {
      const off = subscribe(callback);
      callback({ state: "disconnected", generation: 2 });
      callback({ state: "connected", generation: 3 });
      return off;
    });
    await store.settingsStore.load();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    expect(store.settingsStore.current).toMatchObject(value(15));
  });

  it("publishes only newer event response B when A resolves last, including sounds", async () => {
    await start();
    const a = deferred<unknown>();
    const b = deferred<unknown>();
    fake.onInvoke("get_settings", () => a.promise);
    const load = store.settingsStore.load();
    await settle();
    fake.onInvoke("get_settings", () => b.promise);
    fake.emitFromBackend("room_idle_confirmation_updated", {});
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    b.resolve(value(45, false));
    await settle();
    expect(store.settingsStore.current).toMatchObject(value(45, false));
    expect(setSoundsEnabled).toHaveBeenCalledExactlyOnceWith(false);
    a.resolve(value(1, true));
    await load;
    expect(store.settingsStore.current).toMatchObject(value(45, false));
    expect(setSoundsEnabled).toHaveBeenCalledExactlyOnceWith(false);
  });

  it.each(["disconnect", "generation", "snapshot"])("suppresses pending publication on %s and recovers", async mode => {
    await start();
    await store.settingsStore.load();
    setSoundsEnabled.mockClear();
    const pending = deferred<unknown>();
    fake.onInvoke("get_settings", () => pending.promise);
    const load = store.settingsStore.load();
    await settle();
    const newer = deferred<unknown>();
    fake.onInvoke("get_settings", () => newer.promise);
    if (mode === "snapshot") {
      vi.spyOn(fake, "connectionState").mockReturnValue({ state: "connected", generation: 1 });
    } else {
      fake.setConnectionState({ state: mode === "disconnect" ? "disconnected" : "connected", generation: 1 });
    }
    pending.resolve(value(1, false));
    await load;
    await settle();
    expect(store.settingsStore.current).toMatchObject(value(15));
    expect(setSoundsEnabled).not.toHaveBeenCalled();
    newer.resolve(value(50, false));
    await settle();
    if (mode === "generation") {
      expect(store.settingsStore.current).toMatchObject(value(50, false));
      expect(setSoundsEnabled).toHaveBeenCalledExactlyOnceWith(false);
    }
    fake.resolve("get_settings", value(90, true));
    if (mode === "snapshot") vi.mocked(fake.connectionState).mockRestore();
    fake.setConnectionState({ state: "connected", generation: 2 });
    await settle();
    expect(store.settingsStore.current).toMatchObject(value(90));
    expect(setSoundsEnabled).toHaveBeenLastCalledWith(true);
    expect(fake.callsFor("get_settings").length).toBeGreaterThanOrEqual(3);
  });

  it("cleans partial registration failure, logs and retries on explicit load", async () => {
    await start();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const listen = fake.listen.bind(fake);
    const off = vi.fn();
    vi.spyOn(fake, "listen").mockImplementation(async (event, callback, options) => {
      if (event === "room_idle_confirmation_updated") throw new Error("registration failed");
      const unlisten = await listen(event, callback, options);
      return () => { off(); unlisten(); };
    });
    await expect(store.settingsStore.load()).rejects.toThrow("registration failed");
    expect(off).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalled();
    expect(fake.callsFor("get_settings")).toHaveLength(0);
    fake.emitFromBackend("coding_agent_settings_updated", {});
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(0);
    vi.mocked(fake.listen).mockRestore();
    await store.settingsStore.load();
    expect(fake.callsFor("get_settings")).toHaveLength(1);
    expect(store.settingsStore.current).toMatchObject(value(15));
  });

  it("unlistens a late registration after disposal and remounts once", async () => {
    await start();
    const gate = deferred<() => void>();
    const listen = fake.listen.bind(fake);
    const off = vi.fn();
    vi.spyOn(fake, "listen").mockImplementation(async (event, callback, options) => {
      const unlisten = await listen(event, callback, options);
      if (event === "room_idle_confirmation_updated") await gate.promise;
      return () => { off(); unlisten(); };
    });
    const load = store.settingsStore.load();
    await settle();
    store.__disposeSettingsSubscriptionsForTests();
    gate.resolve(() => undefined);
    await load;
    expect(off).toHaveBeenCalledTimes(2);
    expect(fake.callsFor("get_settings")).toHaveLength(0);
    vi.mocked(fake.listen).mockRestore();
    await store.settingsStore.load();
    fake.emitFromBackend("room_idle_confirmation_updated", {});
    await settle();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    expect(store.settingsStore.current).toMatchObject(value(15));
  });

  it.each(["load", "event", "reconnect"])("retains settings/sounds after failure and retries via %s", async trigger => {
    await start();
    await store.settingsStore.load();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    setSoundsEnabled.mockClear();
    fake.reject("get_settings", "fetch failed");
    await expect(store.settingsStore.load()).rejects.toBe("fetch failed");
    store.settingsStore.refresh();
    await settle();
    expect(error).toHaveBeenCalled();
    expect(store.settingsStore.current).toMatchObject(value(15));
    expect(setSoundsEnabled).not.toHaveBeenCalled();
    fake.resolve("get_settings", value(120, false));
    if (trigger === "load") await store.settingsStore.load();
    if (trigger === "event") fake.emitFromBackend("room_idle_confirmation_updated", {});
    if (trigger === "reconnect") fake.setConnectionState({ state: "connected", generation: 1 });
    await settle();
    expect(store.settingsStore.current).toMatchObject(value(120, false));
    expect(setSoundsEnabled).toHaveBeenCalledExactlyOnceWith(false);
    expect(fake.callsFor("get_settings")).toHaveLength(4);
  });

  it("disposal blocks pending settings and sounds and cleans all three listeners", async () => {
    await start();
    await store.settingsStore.load();
    setSoundsEnabled.mockClear();
    const pending = deferred<unknown>();
    fake.onInvoke("get_settings", () => pending.promise);
    const load = store.settingsStore.load();
    await settle();
    store.__disposeSettingsSubscriptionsForTests();
    pending.resolve(value(1, false));
    await load;
    fake.emitFromBackend("room_idle_confirmation_updated", {});
    fake.emitFromBackend("coding_agent_settings_updated", {});
    fake.setConnectionState({ state: "connected", generation: 1 });
    await settle();
    expect(store.settingsStore.current).toBeNull();
    expect(setSoundsEnabled).not.toHaveBeenCalled();
    expect(fake.callsFor("get_settings")).toHaveLength(2);
    fake.resolve("get_settings", value(60));
    await store.settingsStore.load();
    expect(store.settingsStore.current).toMatchObject(value(60));
    expect(setSoundsEnabled).toHaveBeenCalledExactlyOnceWith(true);
  });
});

describe("settingsStore bootstrap failures", () => {
  it("keeps listeners after failed bootstrap and lets the next load succeed", async () => {
    const fake = new FakeTransport();
    const restore = __setTransportForTests(fake);
    const store = await import("./settings");
    store.__disposeSettingsSubscriptionsForTests();
    fake.reject("get_settings", "bootstrap failed");
    try {
      await expect(store.settingsStore.load()).rejects.toBe("bootstrap failed");
      expect(store.settingsStore.current).toBeNull();
      fake.resolve("get_settings", value(77));
      await store.settingsStore.load();
      expect(store.settingsStore.current).toMatchObject(value(77));
      expect(fake.callsFor("get_settings")).toHaveLength(2);
      expect(fake.listensFor("room_idle_confirmation_updated")).toHaveLength(1);
    } finally { store.__disposeSettingsSubscriptionsForTests(); restore(); }
  });

  it("cleans both event listeners when connection registration fails", async () => {
    const fake = new FakeTransport();
    const restore = __setTransportForTests(fake);
    const store = await import("./settings");
    store.__disposeSettingsSubscriptionsForTests();
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    vi.spyOn(fake, "onConnectionState").mockImplementation(() => { throw new Error("connection registration failed"); });
    try {
      await expect(store.settingsStore.load()).rejects.toThrow("connection registration failed");
      fake.emitFromBackend("coding_agent_settings_updated", {});
      fake.emitFromBackend("room_idle_confirmation_updated", {});
      await settle();
      expect(fake.callsFor("get_settings")).toHaveLength(0);
      expect(error).toHaveBeenCalledTimes(1);
      vi.mocked(fake.onConnectionState).mockRestore();
      fake.resolve("get_settings", value(80));
      await store.settingsStore.load();
      expect(store.settingsStore.current).toMatchObject(value(80));
      fake.emitFromBackend("room_idle_confirmation_updated", {});
      await settle();
      expect(fake.callsFor("get_settings")).toHaveLength(2);
    } finally { store.__disposeSettingsSubscriptionsForTests(); restore(); vi.restoreAllMocks(); }
  });
});
