// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { batch, createEffect, createRoot, onCleanup } from "solid-js";
import type { AcWorkgroup, Session, SessionStatus } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { discovery, resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { projectStore } from "./project";
import { sessionsStore } from "./sessions";
import { remoteActivityStore } from "./remote-activity";
import { isRoomSessionBusy, roomIsRawActive } from "../components/room-session-source";
import { acquireRoomActivityObserver, confirmedRoomIsActive, roomActivityKey } from "./room-activity";

const project = "C:/Project";
function room(path = `${project}/.ac/room-1`): AcWorkgroup {
  return { name: "room-1", path, task: null, agents: [
    { name: "coord", path: `${path}/__agent_coord`, repoPaths: [`${path}/repo-main`], isCoordinator: true },
    { name: "member", path: `${path}/__agent_member`, repoPaths: [`${path}/repo-member`], isCoordinator: false },
  ] };
}
function live(group: AcWorkgroup, status: SessionStatus = "running", overrides: Partial<Session> = {}): Session {
  return session({ id: group.path, name: `${group.name}/coord`, workingDirectory: group.agents[0].path, status, ...overrides });
}
const flush = async () => { await Promise.resolve(); };

describe("room activity observer actual Solid lifecycle", () => {
  const disposals: (() => void)[] = [];
  let fake: FakeTransport;
  let restore: () => void;
  let group: AcWorkgroup;
  beforeEach(async () => {
    resetUiStoresForTests();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] });
    group = room();
    fake = new FakeTransport();
    fake.resolve("new_project", { path: project, registered: true, created: false });
    fake.resolve("discover_project", discovery({ workgroups: [group] }));
    restore = __setTransportForTests(fake);
    await projectStore.createAndLoad(project);
  });
  afterEach(() => {
    for (const dispose of disposals.splice(0).reverse()) dispose();
    restore();
    resetUiStoresForTests();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });
  function owner() {
    let dispose!: () => void;
    let release!: () => void;
    createRoot((d) => {
      dispose = d;
      release = acquireRoomActivityObserver();
      onCleanup(release);
    });
    disposals.push(dispose);
    return { dispose, release };
  }
  function observe(path = project, target = group) {
    const values: boolean[] = [];
    createRoot((dispose) => {
      disposals.push(dispose);
      createEffect(() => values.push(confirmedRoomIsActive(path, target)));
    });
    return values;
  }
  const active = () => confirmedRoomIsActive(project, group);
  const set = (status: SessionStatus) => sessionsStore.setSessions([live(group, status)]);

  it("seeds idle and empty without grace, and wakes a real effect at exactly 15000", async () => {
    owner();
    const values = observe();
    expect(active()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    set("running");
    expect(values[values.length - 1]).toBe(true);
    set("idle");
    vi.advanceTimersByTime(14999);
    expect(performance.now()).toBe(14999);
    expect(values[values.length - 1]).toBe(true);
    vi.advanceTimersByTime(1);
    await flush();
    expect(values[values.length - 1]).toBe(false);
    expect(values.filter((value, index) => index === 0 || value !== values[index - 1])).toEqual([false, true, false]);
  });

  it("initial active confirms; repeated false/keepalive does not extend its first deadline", () => {
    set("running"); owner();
    const values = observe();
    expect(values).toEqual([true]);
    set("idle");
    vi.advanceTimersByTime(10000);
    sessionsStore.setSessions([live(group, "idle")]);
    sessionsStore.markActivity(group.path);
    vi.advanceTimersByTime(5000);
    expect(values[values.length - 1]).toBe(false);
  });

  it("resume at 14000 cancels even a queued stale callback; a later fall gets a full interval", () => {
    set("running"); owner(); const values = observe();
    const timer = vi.spyOn(globalThis, "setTimeout");
    set("idle");
    const stale = timer.mock.calls[timer.mock.calls.length - 1]![0] as () => void;
    vi.advanceTimersByTime(14000);
    set("running");
    stale();
    expect(values[values.length - 1]).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
    set("idle");
    vi.advanceTimersByTime(14999);
    stale();
    expect(active()).toBe(true);
    vi.advanceTimersByTime(1);
    expect(values[values.length - 1]).toBe(false);
  });

  it("early callbacks reschedule remaining monotonic time", () => {
    set("running"); owner(); const values = observe();
    const native = globalThis.setTimeout;
    vi.spyOn(globalThis, "setTimeout").mockImplementationOnce(((callback: () => void) => native(callback, 1000)) as typeof setTimeout);
    set("idle");
    vi.advanceTimersByTime(1000);
    expect(active()).toBe(true);
    vi.advanceTimersByTime(13999);
    expect(values[values.length - 1]).toBe(true);
    vi.advanceTimersByTime(1);
    expect(values[values.length - 1]).toBe(false);
  });

  it("catalogued gaps and idle session replacement retain the deadline", () => {
    set("running"); owner(); const values = observe();
    sessionsStore.setSessions([]);
    vi.advanceTimersByTime(8000);
    sessionsStore.setSessions([live(group, "idle", { id: "replacement" })]);
    vi.advanceTimersByTime(7000);
    expect(values[values.length - 1]).toBe(false);
  });

  it("catalogue removal invalidates callback and reappearance seeds idle", async () => {
    set("running"); owner();
    const timer = vi.spyOn(globalThis, "setTimeout");
    set("idle"); const stale = timer.mock.calls[timer.mock.calls.length - 1]![0] as () => void;
    fake.resolve("discover_project", discovery());
    await projectStore.reloadProject(project);
    expect(active()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
    set("idle");
    fake.resolve("discover_project", discovery({ workgroups: [group] }));
    await projectStore.reloadProject(project);
    stale();
    expect(active()).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each([false, true])("two independent roots survive either release order (reverse=%s)", (reverse) => {
    set("running");
    const a = owner(), b = owner();
    const values = observe();
    set("idle");
    (reverse ? b : a).dispose();
    (reverse ? b : a).release();
    vi.advanceTimersByTime(15000);
    expect(values[values.length - 1]).toBe(false);
    set("running"); set("idle");
    const timer = vi.spyOn(globalThis, "setTimeout");
    // A new edge supplies a callback that may already have been queued at dispose.
    set("running"); set("idle");
    const stale = timer.mock.calls[timer.mock.calls.length - 1]![0] as () => void;
    (reverse ? a : b).dispose();
    expect(vi.getTimerCount()).toBe(0);
    owner();
    stale();
    expect(active()).toBe(false);
    set("running");
    expect(active()).toBe(true);
  });

  it("getter sees raw true synchronously before sampling flush and never starts a timer", () => {
    owner();
    batch(() => {
      set("running");
      expect(active()).toBe(true);
    });
    expect(vi.getTimerCount()).toBe(0);
    expect(confirmedRoomIsActive(" ", group)).toBe(true);
    set("idle");
    expect(confirmedRoomIsActive(" ", group)).toBe(false);
  });

  it("same-name rooms across projects remain isolated and matching requires both path and name", async () => {
    const otherPath = "C:/Other";
    const other = room(`${otherPath}/.ac/room-1`);
    fake.resolve("new_project", { path: otherPath, registered: true, created: false });
    fake.resolve("discover_project", discovery({ workgroups: [other] }));
    await projectStore.createAndLoad(otherPath);
    sessionsStore.setSessions([live(group)]);
    owner();
    expect(active()).toBe(true);
    expect(confirmedRoomIsActive(otherPath, other)).toBe(false);
    sessionsStore.setSessions([live(other, "running", { name: "wrong/coord" })]);
    expect(roomIsRawActive(other)).toBe(false);
    sessionsStore.setSessions([live(other)]);
    expect(confirmedRoomIsActive(otherPath, other)).toBe(true);
    vi.advanceTimersByTime(15000);

    expect(active()).toBe(false);
    expect(confirmedRoomIsActive(otherPath, other)).toBe(true);
  });

  it("raw policy reacts to Co-managed flags and distinguishes ordinary waiting/exited", () => {
    sessionsStore.setSessions([live(group, "running", { waitingForInput: true, pendingReview: true })]);
    owner(); const values = observe();
    expect(active()).toBe(false);
    sessionsStore.setSessionComanaged(group.path, true);
    expect(values[values.length - 1]).toBe(true);
    sessionsStore.setSessionComanaged(group.path, false);
    vi.advanceTimersByTime(15000);
    expect(values[values.length - 1]).toBe(false);
    for (const status of ["active", "running", "idle", { exited: 0 }] as SessionStatus[]) {
      for (const pendingReview of [false, true]) {
        for (const waitingForInput of [false, true]) {
          const current = live(group, status, { pendingReview, waitingForInput });
          expect(isRoomSessionBusy(current, true)).toBe(typeof status === "string");
          expect(isRoomSessionBusy(current, false)).toBe((status === "active" || status === "running") && !pendingReview && !waitingForInput);
        }
      }
    }
    expect(isRoomSessionBusy(undefined, true)).toBe(false);
    expect(isRoomSessionBusy(null, false)).toBe(false);
  });

  it("only coordinator CI contributes, including configured badges without a session", () => {
    const ci = (index: number, running: boolean) => remoteActivityStore.applyRemoteActivityUpdate({
      repoPaths: [group.agents[index].repoPaths[0]], ciStates: [running ? "running" : "idle"],
      stalenessStates: ["current"], behindBy: [null],
    });
    owner(); const values = observe();
    ci(1, true); expect(active()).toBe(false);
    ci(0, true); expect(values[values.length - 1]).toBe(true);
    ci(0, false); vi.advanceTimersByTime(15000);
    expect(values[values.length - 1]).toBe(false);
  });

  it("normalizes Windows aliases, preserves POSIX case and uses tuple identity", () => {
    const windows = (path: string) => path.replaceAll("/", String.fromCharCode(92));
    expect(roomActivityKey(windows("C:/Project/"), windows("C:/Project/.ac/Room"))).toBe(roomActivityKey("c:/project", "c:/project/.ac/room"));
    expect(roomActivityKey("/Project", "/Room")).not.toBe(roomActivityKey("/project", "/Room"));
    expect(roomActivityKey("/a|/b", "/c")).not.toBe(roomActivityKey("/a", "/b|/c"));
    expect(roomActivityKey("", group.path)).toBeNull();
    expect(roomActivityKey(project, " \t")).toBeNull();
  });
});
