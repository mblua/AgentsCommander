import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import type { AgentUpdateOverviewRow, InstallState } from "../../shared/types";
import { OVERVIEW_REPOLL_MS, agentUpdateOverviewStore as store } from "./agent-update-overview";

const CMD = "get_agent_update_overview";

const checking: InstallState = { status: "checking", seq: 0 };

function installed(version: string, seq: number): InstallState {
  return { status: "installed", version, path: `C:\\bin\\${version}.cmd`, seq };
}

function missing(seq: number, detail = "'codex' was not found on PATH"): InstallState {
  return { status: "missing", detail, seq };
}

function row(key: string, command: string, install: InstallState = checking): AgentUpdateOverviewRow {
  return { key, label: key, command, color: "#10b981", updateCommands: [`${command} update`], install };
}

function installOf(command: string): InstallState[] {
  return (store.state.rows ?? []).filter((entry) => entry.command === command).map((entry) => entry.install);
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** One macrotask: every pending microtask (the refresh loop included) has run. */
async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("agentUpdateOverviewStore (#1551)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    store.resetForTests();
  });

  afterEach(() => {
    store.resetForTests();
    restore();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("open then refresh invokes once and replaces rows", async () => {
    let respond = (): Promise<AgentUpdateOverviewRow[]> => Promise.resolve([row("codex", "codex")]);
    fake.onInvoke(CMD, () => respond());
    store.open();
    expect(store.state).toMatchObject({ rows: null, loading: false, error: null });

    const pending = store.refresh();
    expect(store.state.loading).toBe(true);
    await pending;
    expect(fake.callsFor(CMD)).toEqual([{ cmd: CMD, args: {} }]);
    expect(store.state.rows?.map((entry) => entry.key)).toEqual(["codex"]);
    expect(store.state).toMatchObject({ loading: false, error: null });

    respond = () => Promise.resolve([row("pi", "pi"), row("opencode", "opencode")]);
    await store.refresh();
    expect(fake.callsFor(CMD)).toHaveLength(2);
    expect(store.state.rows?.map((entry) => entry.key)).toEqual(["pi", "opencode"]);
  });

  it("refresh while an invoke is in flight guarantees exactly one trailing invoke", async () => {
    let unsettled = 0;
    let maxUnsettled = 0;
    const gates: Array<(rows: AgentUpdateOverviewRow[]) => void> = [];
    fake.onInvoke(CMD, () => {
      unsettled += 1;
      maxUnsettled = Math.max(maxUnsettled, unsettled);
      return new Promise<AgentUpdateOverviewRow[]>((resolve) => {
        gates.push((rows) => {
          unsettled -= 1;
          resolve(rows);
        });
      });
    });
    store.open();

    const first = store.refresh();
    const second = store.refresh();
    const third = store.refresh();
    expect(second).toBe(first);
    expect(third).toBe(first);
    expect(fake.callsFor(CMD)).toHaveLength(1);

    gates[0]([row("codex", "codex")]);
    await settle();
    // exactly ONE trailing invoke for the two requests that arrived in flight
    expect(fake.callsFor(CMD)).toHaveLength(2);
    gates[1]([row("codex", "codex", installed("1.0", 1))]);
    await first;
    expect(fake.callsFor(CMD)).toHaveLength(2);
    expect(installOf("codex")).toEqual([installed("1.0", 1)]);

    const fourth = store.refresh();
    expect(fake.callsFor(CMD)).toHaveLength(3);
    gates[2]([row("codex", "codex", installed("1.0", 1))]);
    await fourth;
    expect(fake.callsFor(CMD)).toHaveLength(3);
    expect(maxUnsettled).toBe(1);
  });

  it("event before response is kept when its seq is newer", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    store.open();
    store.applyInstallState("codex", missing(3));
    expect(store.state.rows).toBeNull();

    await store.refresh();
    expect(installOf("codex")).toEqual([missing(3)]);
  });

  it("response before event", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    store.open();
    await store.refresh();
    expect(installOf("codex")).toEqual([checking]);

    store.applyInstallState("codex", missing(1));
    expect(installOf("codex")).toEqual([missing(1)]);
  });

  it("an older response never downgrades a newer event state", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", installed("1.0", 4))]));
    store.open();
    store.applyInstallState("codex", installed("2.0", 5));
    await store.refresh();
    expect(installOf("codex")).toEqual([installed("2.0", 5)]);
  });

  it("a newer response overrides an older event state", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", installed("2.0", 3))]));
    store.open();
    store.applyInstallState("codex", installed("1.0", 2));
    await store.refresh();
    expect(installOf("codex")).toEqual([installed("2.0", 3)]);
  });

  it("checking in a response never overwrites a committed state", async () => {
    let respond = (): Promise<AgentUpdateOverviewRow[]> =>
      Promise.resolve([row("codex", "codex", installed("1.0", 1))]);
    fake.onInvoke(CMD, () => respond());
    store.open();
    await store.refresh();
    expect(installOf("codex")).toEqual([installed("1.0", 1)]);

    respond = () => Promise.resolve([row("codex", "codex", checking)]);
    await store.refresh();
    expect(installOf("codex")).toEqual([installed("1.0", 1)]);

    store.applyInstallState("codex", checking);
    expect(installOf("codex")).toEqual([installed("1.0", 1)]);
  });

  it("duplicate-command rows both update", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("pi", "pi"), row("pi-alt", "pi"), row("codex", "codex")]));
    store.open();
    await store.refresh();

    store.applyInstallState("pi", installed("0.84.3", 1));
    expect(installOf("pi")).toEqual([installed("0.84.3", 1), installed("0.84.3", 1)]);
    expect(installOf("codex")).toEqual([checking]);
  });

  it("a newer state replaces the row's install: keys the new state omits do not survive", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex")]));
    store.open();
    await store.refresh();

    store.applyInstallState("codex", installed("1.0", 1));
    store.applyInstallState("codex", missing(2));
    const [install] = installOf("codex");
    expect(install).toEqual(missing(2));
    expect(install).not.toHaveProperty("version");
    expect(install).not.toHaveProperty("path");
  });

  it("re-polls after OVERVIEW_REPOLL_MS while a response contains checking, and not after an all-committed response", async () => {
    vi.useFakeTimers();
    let rows = [row("codex", "codex", checking), row("pi", "pi", installed("0.84.3", 1))];
    fake.onInvoke(CMD, () => Promise.resolve(rows));
    store.open();
    await store.refresh();
    expect(fake.callsFor(CMD)).toHaveLength(1);
    expect(vi.getTimerCount()).toBe(1);

    await vi.advanceTimersByTimeAsync(OVERVIEW_REPOLL_MS - 1);
    expect(fake.callsFor(CMD)).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(fake.callsFor(CMD)).toHaveLength(2);
    // the second response still reports checking: armed again
    expect(vi.getTimerCount()).toBe(1);

    rows = [row("codex", "codex", installed("1.0", 2)), row("pi", "pi", installed("0.84.3", 1))];
    await vi.advanceTimersByTimeAsync(OVERVIEW_REPOLL_MS);
    expect(fake.callsFor(CMD)).toHaveLength(3);
    expect(installOf("codex")).toEqual([installed("1.0", 2)]);
    // an all-committed response arms nothing
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(OVERVIEW_REPOLL_MS * 3);
    expect(fake.callsFor(CMD)).toHaveLength(3);
  });

  it("close cancels a pending re-poll and a new open starts without one", async () => {
    vi.useFakeTimers();
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    store.open();
    await store.refresh();
    expect(vi.getTimerCount()).toBe(1);

    store.close();
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(OVERVIEW_REPOLL_MS * 2);
    expect(fake.callsFor(CMD)).toHaveLength(1);

    store.open();
    await vi.advanceTimersByTimeAsync(OVERVIEW_REPOLL_MS * 2);
    expect(fake.callsFor(CMD)).toHaveLength(1);
    expect(store.state.rows).toBeNull();
  });

  it("a response from a closed open is discarded", async () => {
    const gate = deferred<AgentUpdateOverviewRow[]>();
    fake.onInvoke(CMD, () => gate.promise);
    store.open();
    const pending = store.refresh();
    expect(store.state.loading).toBe(true);

    store.close();
    store.open();
    expect(store.state.loading).toBe(false);

    gate.resolve([row("codex", "codex")]);
    await pending;
    expect(store.state).toMatchObject({ rows: null, loading: false, error: null });
  });

  it("failure sets error and keeps rows (null on a first failure)", async () => {
    let fail = true;
    fake.onInvoke(CMD, () =>
      fail ? Promise.reject(new Error("boom")) : Promise.resolve([row("codex", "codex")])
    );
    store.open();

    await store.refresh();
    expect(store.state).toMatchObject({ rows: null, loading: false, error: "boom" });

    fail = false;
    await store.refresh();
    expect(store.state.error).toBeNull();
    expect(store.state.rows?.map((entry) => entry.key)).toEqual(["codex"]);

    fail = true;
    await store.refresh();
    expect(store.state.error).toBe("boom");
    expect(store.state.rows?.map((entry) => entry.key)).toEqual(["codex"]);
    expect(store.state.loading).toBe(false);
  });

  it("open clears the latest map: an event before open() does not survive it", async () => {
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    store.applyInstallState("codex", missing(3));
    store.open();
    await store.refresh();
    expect(installOf("codex")).toEqual([checking]);
  });
});
