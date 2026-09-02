// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  __resetIpcBlackBoxForTests,
  harvestIpcBlackBox,
  installIpcBlackBox,
  noteEvent,
  noteInvokeSettle,
  noteInvokeStart,
  type IpcBlackBoxRecord,
  type StoredBlackBox,
} from "./ipc-blackbox";

// The probe listener is registered through a dynamic
// `import("@tauri-apps/api/event")` inside a try/catch. In jsdom that import
// resolves but `listen` throws, and the throw is swallowed, so without this
// double no test could ever reach the callback.
// `@tauri-apps/api/webviewWindow` is deliberately left unmocked: its throw is
// the `"web"` label path, which is what every test here exercises.
const mocks = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

const CURRENT_KEY = "ac.ipc.bb.cur.web";
const ROTATED_KEY = "ac.ipc.bb.prev.web";
const TICK_MS = 1_000;

/** The 23 fields of phase 1's `BlackBoxRecord`, camelCase, in schema order. */
const SCHEMA_FIELDS = [
  "v",
  "label",
  "windowType",
  "startedAtMs",
  "writtenAtMs",
  "tickSeq",
  "rafSeq",
  "lastRafAtMs",
  "visible",
  "closedCleanly",
  "lastPointerAtMs",
  "lastEventAtMs",
  "lastEventName",
  "probeSeq",
  "probeAtMs",
  "sent",
  "settled",
  "lastSettledAtMs",
  "lastSentAtMs",
  "pendingTotal",
  "overdueTotal",
  "pending",
  "perCommand",
];

let visibilityState: DocumentVisibilityState = "visible";

const readRecord = (key = CURRENT_KEY): IpcBlackBoxRecord => {
  const raw = localStorage.getItem(key);
  expect(raw).not.toBeNull();
  return JSON.parse(raw as string) as IpcBlackBoxRecord;
};

describe("ipc black box", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    __resetIpcBlackBoxForTests();
    mocks.listen.mockReset();
    mocks.listen.mockResolvedValue(() => undefined);
    visibilityState = "visible";
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => visibilityState,
    });
  });

  afterEach(() => {
    __resetIpcBlackBoxForTests();
    Reflect.deleteProperty(document, "visibilityState");
    localStorage.clear();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("records a call from start to settle", async () => {
    await installIpcBlackBox();

    const sentAt = Date.now();
    const id = noteInvokeStart("pty_write");
    vi.advanceTimersByTime(TICK_MS);

    const inFlight = readRecord();
    expect(inFlight.sent).toBe(1);
    expect(inFlight.settled).toBe(0);
    expect(inFlight.pendingTotal).toBe(1);
    expect(inFlight.pending.map((entry) => entry.cmd)).toEqual(["pty_write"]);
    expect(inFlight.perCommand.pty_write).toEqual([1, 0]);
    // Asserted BEFORE the settle: the whole point of the field is that it does
    // not wait for one.
    expect(inFlight.lastSentAtMs).toBe(sentAt);
    expect(inFlight.lastSettledAtMs).toBe(0);

    noteInvokeSettle(id);
    vi.advanceTimersByTime(TICK_MS);

    const done = readRecord();
    expect(done.sent).toBe(1);
    expect(done.settled).toBe(1);
    expect(done.pendingTotal).toBe(0);
    expect(done.pending).toEqual([]);
    expect(done.perCommand.pty_write).toEqual([1, 1]);
    expect(done.lastSentAtMs).toBe(sentAt);
    expect(done.lastSettledAtMs).toBeGreaterThan(0);
  });

  it("marks a call overdue past the threshold", async () => {
    await installIpcBlackBox();

    noteInvokeStart("get_sessions");
    vi.advanceTimersByTime(6_000);

    const record = readRecord();
    expect(record.pending).toHaveLength(1);
    expect(record.pending[0].cmd).toBe("get_sessions");
    expect(record.pending[0].ageMs).toBeGreaterThanOrEqual(5_000);
    expect(record.pending[0].overdue).toBe(true);
    expect(record.overdueTotal).toBe(1);
  });

  it("never marks the three file-dialog commands overdue", async () => {
    await installIpcBlackBox();

    noteInvokeStart("pick_folder");
    noteInvokeStart("spec_board_pick_open");
    noteInvokeStart("spec_board_pick_save");
    vi.advanceTimersByTime(60_000);

    const record = readRecord();
    expect(record.pending.map((entry) => [entry.cmd, entry.overdue])).toEqual([
      ["pick_folder", false],
      ["spec_board_pick_open", false],
      ["spec_board_pick_save", false],
    ]);
    expect(record.pending[0].ageMs).toBeGreaterThanOrEqual(60_000);
    expect(record.pendingTotal).toBe(3);
    expect(record.overdueTotal).toBe(0);
  });

  it("counts every overdue call even when pending is capped and keeps the oldest", async () => {
    await installIpcBlackBox();

    const ids: number[] = [];
    for (let i = 0; i < 40; i += 1) {
      ids.push(noteInvokeStart(`cmd_${i}`));
      vi.advanceTimersByTime(1_000);
    }
    // Past OVERDUE_MS beyond the LAST of the 40, or not all of them are overdue
    // and the count assertion is not the one it claims to be.
    vi.advanceTimersByTime(5_000);

    const record = readRecord();
    expect(record.pending).toHaveLength(32);
    expect(record.pendingTotal).toBe(40);
    expect(record.overdueTotal).toBe(40);
    expect(record.pending.every((entry) => entry.overdue)).toBe(true);
    // The cap KEEPS the oldest outstanding calls, in issue order, which is what
    // lets phase 1 read the oldest overdue entry off the front of the array.
    expect(record.pending.map((entry) => entry.id)).toEqual(ids.slice(0, 32));
    // An emitter with the right ids and the wrong `ageMs` values - all equal, or
    // reversed - passes the assertion above and fails only this one, and this is
    // what pins the `writtenAtMs - ageMs` identity phase 1 reads.
    const ages = record.pending.map((entry) => entry.ageMs);
    expect(record.pending[0].ageMs).toBe(Math.max(...ages));
  });

  it("rotates the previous run's record before the first write", async () => {
    localStorage.setItem(CURRENT_KEY, '{"previous":"run"}');

    await installIpcBlackBox();

    expect(localStorage.getItem(ROTATED_KEY)).toBe('{"previous":"run"}');
    expect(localStorage.getItem(CURRENT_KEY)).toBeNull();

    vi.advanceTimersByTime(TICK_MS);

    const written = localStorage.getItem(CURRENT_KEY);
    expect(written).not.toBeNull();
    expect(written).not.toBe('{"previous":"run"}');
    expect(localStorage.getItem(ROTATED_KEY)).toBe('{"previous":"run"}');
  });

  it("writes every field of the schema", async () => {
    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);

    const record = readRecord();
    // A round-trip cannot see a renamed key; the key set can.
    expect(Object.keys(record)).toEqual(SCHEMA_FIELDS);
    expect(SCHEMA_FIELDS).toHaveLength(23);
    expect(record.v).toBe(1);
    expect(record.label).toBe("web");
    expect(record.windowType).toBe("main");
    expect(record.closedCleanly).toBe(false);
    expect(record.lastSentAtMs).toBe(0);
  });

  it("seeds visible from document.visibilityState at install", async () => {
    visibilityState = "visible";
    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);
    // No `visibilitychange` is dispatched in either seed case: this pins the
    // VALUE, not its consequence.
    expect(readRecord().visible).toBe(true);

    __resetIpcBlackBoxForTests();
    localStorage.clear();

    visibilityState = "hidden";
    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);
    expect(readRecord().visible).toBe(false);

    visibilityState = "visible";
    document.dispatchEvent(new Event("visibilitychange"));
    vi.advanceTimersByTime(TICK_MS);
    expect(readRecord().visible).toBe(true);
  });

  it("harvest sends every other key and deletes exactly what the backend asks for", async () => {
    localStorage.setItem(CURRENT_KEY, '{"previous":"web"}');
    localStorage.setItem("ac.ipc.bb.cur.other", '{"sibling":"current"}');
    localStorage.setItem("ac.ipc.bb.prev.other", '{"sibling":"rotated"}');

    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);

    const seen: StoredBlackBox[][] = [];
    const send = vi.fn(async (records: StoredBlackBox[]) => {
      seen.push(records);
      return [ROTATED_KEY, "ac.ipc.bb.prev.other"];
    });

    await harvestIpcBlackBox(send);

    expect(send).toHaveBeenCalledTimes(1);
    // The live `cur.web` is excluded, which is deterministic only because
    // harvest awaits the install (and therefore the rotation) first.
    expect(seen[0].map((record) => record.key).sort()).toEqual([
      "ac.ipc.bb.cur.other",
      "ac.ipc.bb.prev.other",
      ROTATED_KEY,
    ]);
    expect(seen[0].find((record) => record.key === ROTATED_KEY)?.json).toBe('{"previous":"web"}');

    expect(localStorage.getItem(ROTATED_KEY)).toBeNull();
    expect(localStorage.getItem("ac.ipc.bb.prev.other")).toBeNull();
    expect(localStorage.getItem("ac.ipc.bb.cur.other")).toBe('{"sibling":"current"}');
    expect(localStorage.getItem(CURRENT_KEY)).not.toBeNull();
  });

  it("harvest survives a rejecting sender", async () => {
    localStorage.setItem("ac.ipc.bb.prev.other", '{"sibling":"rotated"}');

    await installIpcBlackBox();
    const send = vi.fn(() => Promise.reject(new Error("command unavailable")));

    await expect(harvestIpcBlackBox(send)).resolves.toBeUndefined();

    expect(send).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem("ac.ipc.bb.prev.other")).toBe('{"sibling":"rotated"}');
  });

  it("the heartbeat survives a throwing setItem", async () => {
    await installIpcBlackBox();

    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("QuotaExceededError");
    });

    vi.advanceTimersByTime(TICK_MS);
    expect(setItem).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(CURRENT_KEY)).toBeNull();

    vi.advanceTimersByTime(TICK_MS);
    expect(readRecord().tickSeq).toBe(2);
  });

  it("records a silence probe without moving lastEventAtMs", async () => {
    let captured: ((event: { payload: { seq: number; backendNowMs: number } }) => void) | null =
      null;
    mocks.listen.mockImplementation((_event: string, callback: unknown) => {
      captured = callback as typeof captured;
      return Promise.resolve(() => undefined);
    });

    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);

    const before = readRecord();
    expect(before.probeSeq).toBeNull();
    expect(before.probeAtMs).toBeNull();
    expect(before.lastEventAtMs).toBe(0);

    expect(mocks.listen).toHaveBeenCalledWith("ipc_silence_probe", expect.any(Function));
    expect(captured).not.toBeNull();
    const probeAt = Date.now();
    (captured as unknown as (event: { payload: { seq: number; backendNowMs: number } }) => void)({
      payload: { seq: 7, backendNowMs: 1234 },
    });
    vi.advanceTimersByTime(TICK_MS);

    const after = readRecord();
    expect(after.probeSeq).toBe(7);
    expect(after.probeAtMs).toBe(probeAt);
    // The probe is counted separately from ordinary traffic.
    expect(after.lastEventAtMs).toBe(0);
    expect(after.lastEventName).toBe("");
  });

  it("noteEvent records the last backend event", async () => {
    await installIpcBlackBox();

    const eventAt = Date.now();
    noteEvent("pty_output");
    vi.advanceTimersByTime(TICK_MS);

    const record = readRecord();
    expect(record.lastEventAtMs).toBe(eventAt);
    expect(record.lastEventName).toBe("pty_output");
  });

  it("marks this window's record cleanly closed instead of removing it", async () => {
    await installIpcBlackBox();
    vi.advanceTimersByTime(TICK_MS);

    const ticked = readRecord();
    expect(ticked.closedCleanly).toBe(false);

    localStorage.setItem(ROTATED_KEY, '{"previous":"run"}');
    vi.advanceTimersByTime(500);
    window.dispatchEvent(new Event("pagehide"));

    const closed = readRecord();
    expect(closed.closedCleanly).toBe(true);
    // The handler wrote synchronously, inside the handler, so the flag reaches
    // localStorage before teardown.
    expect(closed.writtenAtMs).toBeGreaterThan(ticked.writtenAtMs);
    expect(localStorage.getItem(ROTATED_KEY)).toBe('{"previous":"run"}');

    // Second case, and it is the one that stops this test pinning the round 2
    // defect: deleting the record would erase the evidence for rows (b), (c)
    // and (d), which are reached only when the task loop is alive.
    __resetIpcBlackBoxForTests();
    localStorage.clear();

    await installIpcBlackBox();
    noteInvokeStart("get_sessions");
    vi.advanceTimersByTime(6_000);
    expect(readRecord().overdueTotal).toBe(1);

    window.dispatchEvent(new Event("pagehide"));

    const survivor = readRecord();
    expect(survivor.closedCleanly).toBe(true);
    expect(survivor.pending).toHaveLength(1);
    expect(survivor.pending[0].cmd).toBe("get_sessions");
    expect(survivor.overdueTotal).toBe(1);
  });
});
