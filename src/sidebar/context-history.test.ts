// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

import { __setTransportForTests } from "../shared/ipc";
import { FakeTransport } from "../shared/testing/fake-transport";
import type {
  SessionWindowBucket,
  SidebarEntry,
  SidebarMeansQuery,
  SidebarMeansResponse,
} from "../shared/context-history";
import {
  MAX_ACTIVE_KEYS,
  REFRESH_INTERVAL_MS,
  REQUEST_WATCHDOG_MS,
  FOCUS_DEBOUNCE_MS,
  __contextHistoryStatsForTests as stats,
  __resetContextHistoryForTests,
  acquireContextHistory,
  bucketForSessions,
  refreshContextHistoryOnHover,
} from "./context-history";

// #3532 P10 - ownership and request discipline of the sidebar owner. The
// backend computes every mean; these tests only check what is consumed.

const CMD = "get_context_history_sidebar_means";
const P = "C:/proj";
const uuid = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;

const bucket = (
  requestedSessions: number,
  mean: number | null,
  count: number,
  sessions: number,
): SessionWindowBucket => ({
  requestedSessions, recordsCount: count, positiveSum: (mean ?? 0) * count, mean,
  sessionsCount: sessions, recordsPerSession: sessions === 0 ? null : count / sessions,
  incomplete: requestedSessions === 300,
});
const known = (id: string, agentKey = "dev"): SidebarEntry => ({
  runtimeSessionId: id,
  scope: { kind: "known", agentKey, teamIds: ["t1"] },
  sessionWindows: [bucket(25, 12.5, 2, 2), bucket(100, 20, 3, 3), bucket(300, null, 0, 0)],
});

interface Pending {
  query: SidebarMeansQuery;
  resolve: (r: SidebarMeansResponse) => void;
  reject: (e: unknown) => void;
}
const acq = (n: number, projectPath = P) =>
  acquireContextHistory({ projectPath, runtimeSessionId: uuid(n) });
const neutral = (id: string, kind: "ambiguous" | "unknown", teamIds: string[]): SidebarEntry => ({
  runtimeSessionId: id,
  scope: { kind, agentKey: null, teamIds },
  sessionWindows: null,
});

describe("sidebar context-history owner (#3532 P10)", () => {
  let fake: FakeTransport;
  let restore: () => void;
  let pending: Pending[];
  const settle = () => vi.advanceTimersByTimeAsync(0);
  const queries = () =>
    fake.callsFor(CMD).map((c) => (c.args as { request: SidebarMeansQuery }).request);
  const answer = (i: number, entries?: SidebarEntry[]) => {
    const p = pending[i];
    p.resolve({
      schemaVersion: 2,
      generatedAtUtcMs: 1234,
      entries: entries ?? p.query.runtimeSessionIds.map((id) => known(id)),
    });
  };

  beforeEach(() => {
    vi.useFakeTimers();
    platform.tauri = true;
    pending = [];
    fake = new FakeTransport();
    fake.onInvoke(CMD, (args) => new Promise((resolve, reject) => {
      pending.push({ query: (args as { request: SidebarMeansQuery }).request, resolve, reject });
    }));
    restore = __setTransportForTests(fake);
  });

  afterEach(() => {
    __resetContextHistoryForTests();
    restore();
    vi.useRealTimers();
  });

  it("Root, browser, missing or non-UUID ids stay neutral without a request", async () => {
    const handles = [
      acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1), isRoot: true }),
      acquireContextHistory({ projectPath: P, runtimeSessionId: undefined }),
      acquireContextHistory({ projectPath: P, runtimeSessionId: "not-a-uuid" }),
    ];
    platform.tauri = false;
    handles.push(acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(2) }));
    await settle();
    for (const h of handles) expect(h.state()).toEqual({ status: "unavailable" });
    expect(fake.callsFor(CMD)).toHaveLength(0);
    expect(stats()).toMatchObject({ projects: 0, timers: 0, listening: false });
  });

  it("coalesces rows of one microtask and shares one key across rooms", async () => {
    const a = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    const b = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    const c = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(2) });
    expect(a.state()).toEqual({ status: "loading" });
    await settle();
    expect(queries()).toEqual([{ projectPath: P, runtimeSessionIds: [uuid(1), uuid(2)] }]);
    answer(0, [known(uuid(1)), known(uuid(2), "other")]);
    await settle();
    const s = a.state();
    expect(s.status).toBe("ready");
    expect(b.state()).toBe(s);
    if (s.status !== "ready") throw new Error("not ready");
    expect(s.generatedAtUtcMs).toBe(1234);
    expect(s.sessionWindows.map((b) => b.requestedSessions)).toEqual([25, 100, 300]);
    expect(s.sessionWindows[0].mean).toBe(12.5);
    expect(s.sessionWindows[2]).toMatchObject({
      mean: null, recordsCount: 0, recordsPerSession: null, incomplete: true,
    });
    expect(bucketForSessions(s.sessionWindows, 100)).toBe(s.sessionWindows[1]);
    expect(bucketForSessions(s.sessionWindows, undefined)).toBe(s.sessionWindows[2]);
    expect(bucketForSessions(s.sessionWindows, 7)).toBe(s.sessionWindows[2]);
    expect(bucketForSessions([], 25)).toBeNull();
    const other = c.state();
    expect(other.status === "ready" && other.scope.agentKey).toBe("other");
  });

  it("turns a known entry without session windows into an error, never legacy means", async () => {
    const a = acq(1);
    const b = acq(2);
    await settle();
    // Deliberately malformed IPC reply: bypasses the required-field type on purpose.
    const legacy = { ...known(uuid(1)), sessionWindows: undefined } as unknown as SidebarEntry;
    answer(0, [legacy, { ...known(uuid(2)), sessionWindows: [] }]);
    await settle();
    expect(a.state()).toEqual({ status: "error", message: "No session windows in response" });
    expect(b.state()).toEqual({ status: "error", message: "No session windows in response" });
  });

  it("maps ambiguous and unknown to neutral states with text, never zero", async () => {
    const a = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    const u = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(2) });
    const m = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(3) });
    await settle();
    answer(0, [
      neutral(uuid(1), "ambiguous", ["a", "b"]),
      neutral(uuid(2), "unknown", []),
    ]);
    await settle();
    expect(a.state()).toMatchObject({ status: "ambiguous", generatedAtUtcMs: 1234 });
    expect(u.state()).toMatchObject({ status: "unknown" });
    expect((u.state() as { text: string }).text).not.toBe("");
    expect(m.state().status).toBe("error");
  });

  it("splits 101 keys into batches of 100 and 1 with one request in flight", async () => {
    for (let i = 1; i <= 101; i++) acq(i);
    await settle();
    expect(queries()).toHaveLength(1);
    expect(queries()[0].runtimeSessionIds).toHaveLength(100);
    expect(new Set(queries()[0].runtimeSessionIds).size).toBe(100);
    expect(stats()).toMatchObject({ inflight: 1, queued: 1 });
    answer(0);
    await settle();
    expect(queries()).toHaveLength(2);
    expect(queries()[1].runtimeSessionIds).toEqual([uuid(101)]);
  });

  it("caps active keys at 4096; extra rows stay neutral", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    for (let i = 1; i <= MAX_ACTIVE_KEYS; i++) acq(i);
    const extra = acq(MAX_ACTIVE_KEYS + 1);
    expect(extra.state()).toEqual({ status: "unavailable" });
    expect(stats().activeKeys).toBe(MAX_ACTIVE_KEYS);
    expect(warn).toHaveBeenCalledTimes(1);
    await settle();
    expect(stats().queued).toBeLessThanOrEqual(MAX_ACTIVE_KEYS);
    // A 5s reply per batch: still one request at a time and the timer adds nothing.
    for (let n = 0; n < 41; n++) {
      expect(stats().inflight).toBe(1);
      await vi.advanceTimersByTimeAsync(5_000);
      answer(n);
      await settle();
    }
    expect(queries()).toHaveLength(41);
    expect(stats().queued).toBe(0);
    warn.mockRestore();
  });

  it("an IPC error or a hung request does not block the project", async () => {
    const a = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    await settle();
    pending[0].reject({ code: "Busy", message: "busy" });
    await settle();
    expect(a.state()).toEqual({ status: "error", message: "Busy" });
    const b = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(2) });
    await settle();
    expect(queries()).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(REQUEST_WATCHDOG_MS);
    expect(b.state()).toEqual({ status: "error", message: "Timeout" });
    expect(stats().inflight).toBe(0);
    answer(1);
    await settle();
    expect(b.state().status).toBe("error");
  });

  it("refreshes every 30s while visible, skips while pending or hidden", async () => {
    acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    await settle();
    // Pending: nothing re-queues before the watchdog frees the slot.
    await vi.advanceTimersByTimeAsync(REQUEST_WATCHDOG_MS - 1);
    expect(queries()).toHaveLength(1);
    answer(0);
    await settle();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS - REQUEST_WATCHDOG_MS + 1);
    expect(queries()).toHaveLength(2);
    answer(1);
    await settle();
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS);
    expect(queries()).toHaveLength(3);
    answer(2);
    await settle();
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS * 3);
    expect(queries()).toHaveLength(3);
    hidden.mockReturnValue(false);
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("focus"));
    await vi.advanceTimersByTimeAsync(FOCUS_DEBOUNCE_MS);
    expect(queries()).toHaveLength(4);
    hidden.mockRestore();
  });

  it("hover refreshes only data older than 30s", async () => {
    const scope = { projectPath: P, runtimeSessionId: uuid(1) };
    acquireContextHistory(scope);
    await settle();
    answer(0);
    await settle();
    refreshContextHistoryOnHover(scope);
    await settle();
    expect(queries()).toHaveLength(1);
    vi.setSystemTime(Date.now() + REFRESH_INTERVAL_MS + 1);
    refreshContextHistoryOnHover(scope);
    refreshContextHistoryOnHover(scope);
    await settle();
    expect(queries()).toHaveLength(2);
  });

  it("last release clears timer, queue and listeners; a late reply recreates nothing", async () => {
    const a = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    const b = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    await settle();
    a.release();
    a.release();
    expect(stats().activeKeys).toBe(1);
    b.release();
    expect(stats()).toEqual({
      projects: 0, activeKeys: 0, queued: 0, inflight: 0, timers: 0, watchdogs: 0,
      listening: false,
    });
    answer(0);
    await settle();
    expect(stats().projects).toBe(0);
    expect(b.state()).toEqual({ status: "loading" });
    await vi.advanceTimersByTimeAsync(REFRESH_INTERVAL_MS * 2);
    expect(queries()).toHaveLength(1);
  });

  it("a new generation ignores the reply of the previous one", async () => {
    const old = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    await settle();
    old.release();
    const fresh = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    await settle();
    expect(queries()).toHaveLength(2);
    answer(0, [{ ...known(uuid(1), "stale") }]);
    await settle();
    expect(fresh.state()).toEqual({ status: "loading" });
    answer(1);
    await settle();
    expect(fresh.state()).toMatchObject({ status: "ready", scope: { agentKey: "dev" } });
  });

  it("keeps projects apart", async () => {
    const a = acquireContextHistory({ projectPath: P, runtimeSessionId: uuid(1) });
    acquireContextHistory({ projectPath: "C:/other", runtimeSessionId: uuid(1) });
    await settle();
    expect(queries().map((q) => q.projectPath).sort()).toEqual(["C:/other", P]);
    a.release();
    expect(stats()).toMatchObject({ projects: 1, timers: 1 });
  });

  // AC-3532-025: success, error and watchdog of an old batch skip a re-acquired key.
  for (const outcome of ["success", "error", "timeout"] as const) {
    it(`a re-acquired key ignores the old batch ${outcome}`, async () => {
      const a = acq(1);
      acq(2);
      await settle();
      a.release();
      const again = acq(1);
      await settle();
      if (outcome === "success") answer(0);
      if (outcome === "error") pending[0].reject({ code: "Busy", message: "busy" });
      if (outcome === "timeout") await vi.advanceTimersByTimeAsync(REQUEST_WATCHDOG_MS);
      await settle();
      expect(again.state()).toEqual({ status: "loading" });
      expect(queries()).toHaveLength(2);
      expect(queries()[1].runtimeSessionIds).toEqual([uuid(1)]);
      answer(1);
      await settle();
      expect(again.state().status).toBe("ready");
    });
  }

  // AC-3532-026: hover never adds a second pending request for a key in flight.
  it("hover on a key in flight does not refresh it again", async () => {
    const scope = { projectPath: P, runtimeSessionId: uuid(1) };
    acquireContextHistory(scope);
    await settle();
    answer(0);
    await settle();
    vi.setSystemTime(Date.now() + REFRESH_INTERVAL_MS + 1);
    refreshContextHistoryOnHover(scope);
    await settle();
    expect(queries()).toHaveLength(2);
    vi.setSystemTime(Date.now() + REFRESH_INTERVAL_MS + 1);
    refreshContextHistoryOnHover(scope);
    refreshContextHistoryOnHover(scope);
    await settle();
    expect(stats().queued).toBe(0);
    answer(1);
    await settle();
    expect(queries()).toHaveLength(2);
  });

  // AC-3532-027: no timer survives the last release while a request hangs.
  it("last release with an unresolved request leaves zero timers", async () => {
    const a = acq(1);
    await settle();
    expect(stats()).toMatchObject({ inflight: 1, watchdogs: 1 });
    expect(vi.getTimerCount()).toBe(2);
    a.release();
    expect(stats()).toMatchObject({ watchdogs: 0, timers: 0 });
    expect(vi.getTimerCount()).toBe(0);
    acq(2);
    await settle();
    __resetContextHistoryForTests();
    expect(vi.getTimerCount()).toBe(0);
  });
});
