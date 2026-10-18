// @vitest-environment jsdom
//
// Pure focus helpers and real shared-observer completion lifecycle.
import { afterEach, beforeEach, describe, it, expect, vi } from "vitest";

vi.hoisted(() => {
  class MockWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;

    readonly url: string;
    binaryType: BinaryType = "blob";
    readyState = MockWebSocket.CLOSED;

    constructor(url: string) {
      this.url = url;
    }

    send(): void {}

    close(): void {
      this.readyState = MockWebSocket.CLOSED;
    }
  }

  Object.defineProperty(globalThis, "WebSocket", {
    configurable: true,
    writable: true,
    value: MockWebSocket,
  });
});

import {
  allSessionsIdle,
  GRACE_MS,
  hasBusyToIdleTransition,
  isBusy,
  pruneExpiredGrace,
  shouldSuppressBeep,
  startTeamIdleWatcher,
  updateGraceOnFocusChange,
} from "./team-idle-watcher";
import { playTeamIdleBeep } from "../../shared/sound";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  session,
  resetUiStoresForTests,
} from "../../shared/testing/ui-harness";
import { batch, createRoot, onCleanup } from "solid-js";
import type { AcWorkgroup, Session, SessionStatus } from "../../shared/types";
import * as activity from "./room-activity";
import { remoteActivityStore } from "./remote-activity";
import { sessionsStore } from "./sessions";
import { projectStore } from "./project";
import { settingsStore } from "../../shared/stores/settings";

vi.mock("../../shared/sound", () => ({
  playTeamIdleBeep: vi.fn(),
  setSoundsEnabled: vi.fn(),
}));

const focus = vi.hoisted(() => ({
  focused: true, late: false,
  resolve: null as null | ((fn: () => void) => void), unlisten: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isFocused: async () => focus.focused,
    onFocusChanged: () => focus.late
      ? new Promise<() => void>((resolve) => { focus.resolve = resolve; })
      : Promise.resolve(focus.unlisten),
  }),
}));

describe("isBusy (#2453)", () => {
  it("counts a waiting session as busy while it is armed for Co-managed (test 6)", () => {
    const waiting = session({ waitingForInput: true });
    expect(isBusy(waiting, true)).toBe(true);
    expect(isBusy(waiting, false)).toBe(false);
  });

  it("never counts an exited session as busy, armed or not (test 7)", () => {
    const exited = session({ status: { exited: 0 }, waitingForInput: false });
    expect(isBusy(exited, true)).toBe(false);
    expect(isBusy(exited, false)).toBe(false);
  });
});

describe("shouldSuppressBeep (#254)", () => {
  it("suppresses the focused workgroup", () => {
    expect(shouldSuppressBeep("A", "A", new Map(), 0)).toBe(true);
  });

  it("does NOT suppress non-focused workgroups with no grace entry", () => {
    expect(shouldSuppressBeep("B", "A", new Map(), 0)).toBe(false);
  });

  it("suppresses inside an active grace window", () => {
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 2000)).toBe(true);
  });

  it("does NOT suppress once the grace window has expired", () => {
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 5000)).toBe(false);
  });

  it("does NOT suppress at the exact grace boundary (now === until)", () => {
    // The half-open window `now < until` means `now === until` is
    // already outside grace. Locks in the boundary explicitly so a
    // future refactor to `now <= until` would have to flip this test.
    const grace = new Map<string, number>([["A", 4000]]);
    expect(shouldSuppressBeep("A", "B", grace, 4000)).toBe(false);
  });
});

describe("updateGraceOnFocusChange (#254)", () => {
  it("fast switch A→B→A re-arms B's grace and suppresses B inside the window", () => {
    const grace = new Map<string, number>();
    let prev: string | null = null;

    // T=0: initial focus arrives on A. previousFocusedWg was null,
    // so no grace is armed yet.
    prev = updateGraceOnFocusChange(prev, "A", grace, 0, GRACE_MS);
    expect(prev).toBe("A");
    expect(grace.size).toBe(0);

    // T=0: A → B. previousFocusedWg = "A" is left behind: grace
    // armed for A until 0 + 4000 = 4000.
    prev = updateGraceOnFocusChange(prev, "B", grace, 0, GRACE_MS);
    expect(prev).toBe("B");
    expect(grace.get("A")).toBe(4000);

    // T=500: B → A. previousFocusedWg = "B" leaves: grace for B
    // until 500 + 4000 = 4500.
    prev = updateGraceOnFocusChange(prev, "A", grace, 500, GRACE_MS);
    expect(prev).toBe("A");
    expect(grace.get("B")).toBe(4500);

    // T=3000: B is idle, but B is non-focused and still inside its
    // grace window (until 4500) → suppressed.
    expect(shouldSuppressBeep("B", "A", grace, 3000)).toBe(true);

    // T=5000: past B's grace → would beep.
    expect(shouldSuppressBeep("B", "A", grace, 5000)).toBe(false);
  });

  it("alt-tab out (focus → null) arms grace for the previously focused WG", () => {
    const grace = new Map<string, number>();

    // Established focus on A.
    const afterTabOut = updateGraceOnFocusChange(
      "A",
      null,
      grace,
      0,
      GRACE_MS,
    );

    expect(afterTabOut).toBeNull();
    expect(grace.get("A")).toBe(4000);

    // T=2000: A becomes idle while alt-tabbed out → suppressed
    // (grace still active).
    expect(shouldSuppressBeep("A", null, grace, 2000)).toBe(true);
  });

  it("alt-tab back in (focus = A) restores focused-WG suppression for A", () => {
    const grace = new Map<string, number>([["A", 4000]]);

    // The previous tick left focus at null with A's grace armed.
    // Now the window regains focus on A.
    const afterTabIn = updateGraceOnFocusChange(
      null,
      "A",
      grace,
      6000,
      GRACE_MS,
    );

    expect(afterTabIn).toBe("A");
    // previousFocusedWg was null, so no grace was armed for the
    // "left" side — and the existing A grace entry is untouched.
    expect(grace.get("A")).toBe(4000);

    // T=6000: A is focused again → suppressed by the focused-WG rule
    // (independent of A's now-expired grace).
    expect(shouldSuppressBeep("A", "A", grace, 6000)).toBe(true);
  });

  it("no-op when focus does not change", () => {
    const grace = new Map<string, number>();
    const result = updateGraceOnFocusChange("A", "A", grace, 1000, GRACE_MS);
    expect(result).toBe("A");
    expect(grace.size).toBe(0);
  });

  it("background-open: first real tick after listener resolves does not arm grace", () => {
    // Regression guard for the bug where the snapshot tick seeded
    // `previousFocusedWg` from a still-default-true `osFocused`,
    // causing the first real tick (after the async listener
    // resolved `osFocused` to false) to fire a phantom grace
    // window for the active session's WG. The fix moves the
    // updateGraceOnFocusChange call past the snapshot early-return,
    // so the first real tick always sees `previousFocusedWg` of
    // null and the resolved `focusedWg` — null→null is a no-op.
    const grace = new Map<string, number>();
    const result = updateGraceOnFocusChange(null, null, grace, 1000, GRACE_MS);
    expect(result).toBeNull();
    expect(grace.size).toBe(0);
  });
});

describe("hasBusyToIdleTransition (#2109)", () => {
  it("true when a session that was busy is idle now", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", true]]), new Map([["s1", false]])),
    ).toBe(true);
  });

  it("false when a busy session is still busy", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", true]]), new Map([["s1", true]])),
    ).toBe(false);
  });

  it("false when an idle session stays idle", () => {
    expect(
      hasBusyToIdleTransition(new Map([["s1", false]]), new Map([["s1", false]])),
    ).toBe(false);
  });

  it("false when the previously busy key is absent from the current map", () => {
    expect(hasBusyToIdleTransition(new Map([["s1", true]]), new Map())).toBe(false);
  });
});

describe("allSessionsIdle (#2109)", () => {
  it("false when there are no sessions", () => {
    expect(allSessionsIdle(new Map())).toBe(false);
  });

  it("true when every session is idle", () => {
    expect(allSessionsIdle(new Map([["s1", false], ["s2", false]]))).toBe(true);
  });

  it("false when any session is busy", () => {
    expect(allSessionsIdle(new Map([["s1", false], ["s2", true]]))).toBe(false);
  });
});

describe("pruneExpiredGrace (#2109)", () => {
  it("deletes only entries whose grace has expired", () => {
    const grace = new Map([
      ["expired", 200],
      ["boundary", 1000],
      ["active", 1001],
    ]);
    pruneExpiredGrace(grace, 1000);
    expect([...grace.keys()]).toEqual(["active"]);
  });
});

describe("confirmed room completion lifecycle", () => {
  const project = "C:/Project";
  const roomPath = project + "/.ac/room-1";
  const makeRoom = (path = roomPath): AcWorkgroup => ({
    name: "room-1", path, task: null,
    agents: ["coord", "member"].map((name, index) => ({
      name, path: path + "/__agent_" + name,
      repoPaths: [path + "/repo-main"], isCoordinator: index === 0,
    })),
  });
  const live = (status: SessionStatus = "running", overrides: Partial<Session> = {}) => session({
    id: "s1", name: "room-1/coord", workingDirectory: roomPath + "/__agent_coord",
    status, ...overrides,
  });
  const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
  const beep = vi.mocked(playTeamIdleBeep);
  let fake: FakeTransport;
  let restore: () => void;
  const disposals: (() => void)[] = [];
  beforeEach(async () => {
    resetUiStoresForTests();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance", "Date"] });
    beep.mockClear(); focus.focused = true; focus.late = false;
    focus.resolve = null; focus.unlisten.mockClear();
    fake = new FakeTransport();
    fake.resolve("new_project", { path: project, registered: true, created: false });
    fake.resolve("discover_project", discovery({ workgroups: [makeRoom()] }));
    fake.resolve("get_settings", baseSettings({ teamIdleBeepEnabled: true }));
    restore = __setTransportForTests(fake);
    await projectStore.createAndLoad(project);
    await settingsStore.load();
    sessionsStore.setVisibleActiveIdForTests(null);
  });
  afterEach(() => {
    for (const dispose of disposals.splice(0).reverse()) dispose();
    restore(); resetUiStoresForTests(); vi.restoreAllMocks(); vi.useRealTimers();
  });
  async function start(initial = [live()]) {
    sessionsStore.setSessions(initial);
    createRoot((dispose) => {
      disposals.push(dispose); onCleanup(activity.acquireRoomActivityObserver());
    });
    const dispose = startTeamIdleWatcher(); disposals.push(dispose);
    await vi.dynamicImportSettled();
    await flush(); return dispose;
  }
  function ci(running: boolean) {
    remoteActivityStore.applyRemoteActivityUpdate({
      repoPaths: [roomPath + "/repo-main"], ciStates: [running ? "running" : "idle"],
      stalenessStates: ["current"], behindBy: [null],
    });
  }
  function idle() { sessionsStore.setSessions([live("idle")]); }
  function expire() { vi.advanceTimersByTime(15000); }
  async function enabled(value: boolean) {
    fake.resolve("get_settings", baseSettings({ teamIdleBeepEnabled: value }));
    await settingsStore.load();
  }
  it("beeps once at 15000 without another store event; repeated idle cannot replay", async () => {
    await start(); idle(); vi.advanceTimersByTime(14999); expect(beep).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(beep).toHaveBeenCalledTimes(1);
    idle(); expire(); expect(beep).toHaveBeenCalledTimes(1);
  });
  it.each(["empty", "idle", "exited", "replacement", "unmatched"])("%s alone cannot complete", async (kind) => {
    await start(kind === "empty" ? [] : kind === "idle" ? [live("idle")] : [live()]);
    if (kind === "exited") sessionsStore.setSessions([live({ exited: 0 })]);
    if (kind === "replacement") sessionsStore.setSessions([live("idle", { id: "new" })]);
    if (kind === "unmatched") sessionsStore.setSessions([live("idle", { workingDirectory: "C:/elsewhere" })]);
    expire(); expect(beep).not.toHaveBeenCalled();
  });
  it("CI-only falls stay silent", async () => {
    await start([live("idle")]); ci(true); ci(false); expire(); expect(beep).not.toHaveBeenCalled();
  });
  it.each([{ pendingReview: true }, { waitingForInput: true }])("common raw idle flags %j can complete", async (flags) => {
    await start(); sessionsStore.setSessions([live("running", flags)]);
    vi.advanceTimersByTime(14999); expect(beep).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(beep).toHaveBeenCalledTimes(1);
  });
  it("Co-managed prevents completion until disarmed", async () => {
    await start(); sessionsStore.setSessionComanaged("s1", true); idle(); expire();
    expect(beep).not.toHaveBeenCalled(); sessionsStore.setSessionComanaged("s1", false);
    vi.advanceTimersByTime(14999); expect(beep).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(beep).toHaveBeenCalledTimes(1);
  });
  it("existing CI delays until CI fall plus 15s", async () => {
    await start(); ci(true); idle(); expire(); expect(beep).not.toHaveBeenCalled();
    ci(false); vi.advanceTimersByTime(14999); expect(beep).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(beep).toHaveBeenCalledTimes(1);
  });
  it.each([false, true])("new CI after capture cancels, including after original CI ends (%s)", async (existing) => {
    await start(); if (existing) ci(true); idle(); if (existing) ci(false);
    ci(true); ci(false); expire(); expect(beep).not.toHaveBeenCalled();
  });
  it("resumed work cancels at 14s and a later completion starts fresh", async () => {
    await start(); idle(); vi.advanceTimersByTime(14000); sessionsStore.setSessions([live()]);
    vi.advanceTimersByTime(1000); expect(beep).not.toHaveBeenCalled(); idle();
    vi.advanceTimersByTime(14999); expect(beep).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(beep).toHaveBeenCalledTimes(1);
  });
  it.each(["gone", "replaced", "exited", "unmatched"])("contributing ID %s cancels the entire multi-ID latch", async (kind) => {
    const member = (status: SessionStatus) => live(status, {
      id: "s2", name: "room-1/member", workingDirectory: roomPath + "/__agent_member",
    });
    await start([live(), member("running")]);
    batch(() => sessionsStore.setSessions([live("idle"), member("idle")]));
    vi.advanceTimersByTime(5000);
    const invalid = kind === "gone" ? [] : [live(kind === "exited" ? { exited: 0 } : "idle", {
      id: kind === "replaced" ? "replacement" : "s1",
      workingDirectory: kind === "unmatched" ? "C:/wrong" : roomPath + "/__agent_coord",
    })];
    sessionsStore.setSessions([...invalid, member("idle")]);
    expire(); sessionsStore.setSessions([live("idle"), member("idle")]); expire();
    expect(beep).not.toHaveBeenCalled();
  });
  it("same names across projects never share identity, completion or focus", async () => {
    const other = "C:/Other", otherRoom = makeRoom(other + "/.ac/room-1");
    fake.resolve("new_project", { path: other, registered: true, created: false });
    fake.resolve("discover_project", discovery({ workgroups: [otherRoom] }));
    await projectStore.createAndLoad(other);
    const otherLive = live("idle", { id: "other", workingDirectory: otherRoom.agents[0].path });
    await start([live(), otherLive]); sessionsStore.setVisibleActiveIdForTests("other");
    sessionsStore.setSessions([live("idle"), otherLive]);
    sessionsStore.setVisibleActiveIdForTests("other");
    expect(sessionsStore.activeId).toBe("other"); expire();
    expect(beep).toHaveBeenCalledTimes(1);
  });
  it.each([false, true])("focus or recent 4s leave consumes completion without replay (leave=%s)", async (leave) => {
    await start(); sessionsStore.setVisibleActiveIdForTests("s1"); sessionsStore.setSessionWaiting("s1", true);
    vi.advanceTimersByTime(13000);
    if (leave) sessionsStore.setVisibleActiveIdForTests(null);
    vi.advanceTimersByTime(2000); expect(beep).not.toHaveBeenCalled();
    sessionsStore.setVisibleActiveIdForTests(null); expire(); expect(beep).not.toHaveBeenCalled();
  });
  it("focus grace expires independently before the 15s room deadline", async () => {
    await start(); sessionsStore.setVisibleActiveIdForTests("s1"); sessionsStore.setSessionWaiting("s1", true);
    sessionsStore.setVisibleActiveIdForTests(null); expire(); expect(beep).toHaveBeenCalledTimes(1);
  });
  it("disabled clears pending and reseeds; reenable and focus changes cannot replay", async () => {
    await start(); idle(); await enabled(false); expire(); await enabled(true);
    sessionsStore.setVisibleActiveIdForTests("s1"); sessionsStore.setVisibleActiveIdForTests(null);
    expire(); expect(beep).not.toHaveBeenCalled();
    sessionsStore.setSessions([live()]); idle(); expire(); expect(beep).toHaveBeenCalledTimes(1);
  });
  it("catalogue removal cancels pending and idle reappearance cannot replay", async () => {
    await start(); idle(); fake.resolve("discover_project", discovery());
    await projectStore.reloadProject(project); expire();
    fake.resolve("discover_project", discovery({ workgroups: [makeRoom()] }));
    await projectStore.reloadProject(project); expire(); expect(beep).not.toHaveBeenCalled();
  });
  it("dispose clears pending and late listener resolution is unlistened exactly once", async () => {
    focus.late = true; const dispose = await start(); idle(); dispose();
    expect(focus.resolve).not.toBeNull();
    focus.resolve?.(focus.unlisten); await flush(); expire();
    expect(focus.unlisten).toHaveBeenCalledTimes(1); expect(beep).not.toHaveBeenCalled();
  });
  it("latch creation also evaluates already-confirmed-inactive eligibility", async () => {
    await start();
    // Controlled API seam for the unusual publication ordering required by P03.
    const getter = vi.spyOn(activity, "confirmedRoomIsActive").mockReturnValue(false);
    idle(); expect(beep).toHaveBeenCalledTimes(1); getter.mockRestore();
    expire(); expect(beep).toHaveBeenCalledTimes(1);
  });
});
