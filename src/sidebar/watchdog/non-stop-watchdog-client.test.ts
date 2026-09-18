// @vitest-environment jsdom
//
// #777 Non-stop watchdog client. Tests the pure detection (buildSnapshot) against
// seeded stores: the disparity signal is computed with the exact rail code
// (workgroupIsWorking + projectStore.projects), so this proves counter parity and
// the onset/clear flip. The #2180 alarm listener wiring in
// startNonStopWatchdogClient is driven below inside a Solid root with the
// FakeTransport seam; the createEffect + 10s keepalive path stays audited by
// review (team-idle-watcher convention).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot } from "solid-js";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { baseSettings, discovery, resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { defaultGroupsConfig, defaultNonStop, workgroupGroupsStore } from "../stores/workgroup-groups";
import type { NonStopGroupConfig } from "../../shared/types";
import { playNonStopAlarm, stopNonStopAlarm, stopAllNonStopAlarms } from "../../shared/sound";
import { buildSnapshot, startNonStopWatchdogClient } from "./non-stop-watchdog-client";

// Total on purpose: seed() reaches settingsStore.load(), whose last statement is
// setSoundsEnabled(...); a three-key factory would make it undefined and throw.
vi.mock("../../shared/sound", () => ({
  playNonStopAlarm: vi.fn(),
  stopNonStopAlarm: vi.fn(),
  stopAllNonStopAlarms: vi.fn(),
  setSoundsEnabled: vi.fn(),
  primeAudio: vi.fn(),
  playTeamIdleBeep: vi.fn(),
}));

const projectPath = "C:\\Project";
const wgPath = (name: string) => `${projectPath}\\.ac\\${name}`;
const agentPath = (name: string) => `${wgPath(name)}\\__agent_dev-webpage-ui`;

function wgDiscovery() {
  return discovery({
    workgroups: ["wg-1-dev-team", "wg-2-dev-team"].map((name) => ({
      name,
      path: wgPath(name),
      task: null,
      agents: [{ name: "dev-webpage-ui", path: agentPath(name), repoPaths: [], isCoordinator: true }],
    })),
  });
}

function workingSession(wgName: string) {
  return session({
    id: `s-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: agentPath(wgName),
    status: "running",
  });
}

async function seed(nonStop: NonStopGroupConfig | null) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", wgDiscovery());
  fake.resolve("get_project_groups", { ...defaultGroupsConfig(), nonStop });
  const restore = __setTransportForTests(fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await workgroupGroupsStore.ensureLoaded(projectPath);
  return restore;
}

const activeNonStop = (): NonStopGroupConfig => ({
  ...defaultNonStop(),
  show: true,
  regex: "^wg-",
  telegram: { enabled: true, botId: null },
});

describe("#777 nonStopWatchdogClient buildSnapshot", () => {
  let restore: (() => void) | null = null;

  beforeEach(() => {
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    restore?.();
    restore = null;
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  it("flips disparity true<->false as a member goes not-working and recovers (parity with the rail counter)", async () => {
    restore = await seed(activeNonStop());

    // Both members working -> no disparity.
    sessionsStore.setSessions([workingSession("wg-1-dev-team"), workingSession("wg-2-dev-team")]);
    let snap = buildSnapshot();
    expect(snap).toHaveLength(1);
    expect(snap[0]).toMatchObject({ disparity: false, working: 2, total: 2, notWorkingWorkgroups: [] });

    // wg-2 goes not-working (session removed) -> disparity, counter 1/2.
    sessionsStore.setSessions([workingSession("wg-1-dev-team")]);
    snap = buildSnapshot();
    expect(snap[0]).toMatchObject({ disparity: true, working: 1, total: 2, notWorkingWorkgroups: ["wg-2-dev-team"] });

    // Recovery -> disparity clears.
    sessionsStore.setSessions([workingSession("wg-1-dev-team"), workingSession("wg-2-dev-team")]);
    expect(buildSnapshot()[0]).toMatchObject({ disparity: false, working: 2, total: 2 });
  });

  it("does not report when the group is hidden, absent, or has no measures enabled", async () => {
    restore = await seed({ ...activeNonStop(), show: false });
    sessionsStore.setSessions([workingSession("wg-1-dev-team")]);
    expect(buildSnapshot()).toEqual([]); // hidden

    restore?.();
    workgroupGroupsStore.resetForTests();
    // Shown but no measures enabled -> silent by design.
    restore = await seed({ ...defaultNonStop(), show: true, regex: "^wg-" });
    sessionsStore.setSessions([workingSession("wg-1-dev-team")]);
    expect(buildSnapshot()).toEqual([]);

    restore?.();
    workgroupGroupsStore.resetForTests();
    restore = await seed(null); // absent slot
    sessionsStore.setSessions([workingSession("wg-1-dev-team")]);
    expect(buildSnapshot()).toEqual([]);
  });

  it("produces a byte-stable snapshot for unchanged state (the dedupe basis)", async () => {
    restore = await seed(activeNonStop());
    sessionsStore.setSessions([workingSession("wg-1-dev-team")]);
    expect(JSON.stringify(buildSnapshot())).toBe(JSON.stringify(buildSnapshot()));
  });
});

describe("#2180 nonStopWatchdogClient alarm listener", () => {
  let restore: (() => void) | null = null;

  beforeEach(() => {
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
    vi.clearAllMocks();
  });

  afterEach(() => {
    restore?.();
    restore = null;
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  async function seedClient(): Promise<FakeTransport> {
    const transport = new FakeTransport();
    transport.resolve("new_project", { path: projectPath, registered: true, created: false });
    transport.resolve("get_settings", baseSettings());
    transport.resolve("discover_project", wgDiscovery());
    transport.resolve("get_project_groups", { ...defaultGroupsConfig(), nonStop: activeNonStop() });
    transport.resolve("non_stop_report", undefined);
    restore = __setTransportForTests(transport);
    await settingsStore.load();
    await projectStore.createAndLoad(projectPath);
    await workgroupGroupsStore.ensureLoaded(projectPath);
    return transport;
  }

  function startInRoot(): () => void {
    return createRoot((dispose) => {
      startNonStopWatchdogClient();
      return dispose;
    });
  }

  it("registers exactly one listener for the non_stop_alarm event", async () => {
    const fake = await seedClient();
    const dispose = startInRoot();

    expect(fake.listensFor("non_stop_alarm")).toHaveLength(1);

    dispose();
  });

  it("plays a start event with the project path and the reported seconds", async () => {
    const fake = await seedClient();
    const dispose = startInRoot();

    fake.emitFromBackend("non_stop_alarm", {
      projectPath: "C:\\P",
      groupName: "Alert me!",
      seconds: 7,
      action: "start",
    });

    expect(vi.mocked(playNonStopAlarm)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(playNonStopAlarm)).toHaveBeenCalledWith("C:\\P", 7);

    dispose();
  });

  it("stops a stop event by project path and never plays", async () => {
    const fake = await seedClient();
    const dispose = startInRoot();

    fake.emitFromBackend("non_stop_alarm", {
      projectPath: "C:\\P",
      groupName: "",
      seconds: 0,
      action: "stop",
    });

    expect(vi.mocked(stopNonStopAlarm)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(stopNonStopAlarm)).toHaveBeenCalledWith("C:\\P");
    expect(vi.mocked(playNonStopAlarm)).not.toHaveBeenCalled();

    dispose();
  });

  it("cleanup stops every alarm and detaches the listener", async () => {
    const fake = await seedClient();
    const dispose = startInRoot();

    dispose();
    expect(vi.mocked(stopAllNonStopAlarms)).toHaveBeenCalledTimes(1);

    // The unlisten lands one microtask after dispose() via the disposed latch;
    // advanceTimersByTime would not settle it because no timer is involved.
    await Promise.resolve();
    fake.emitFromBackend("non_stop_alarm", {
      projectPath: "C:\\P",
      groupName: "Alert me!",
      seconds: 7,
      action: "start",
    });

    expect(vi.mocked(playNonStopAlarm)).not.toHaveBeenCalled();
    expect(vi.mocked(stopNonStopAlarm)).not.toHaveBeenCalled();
  });

  it("does not play anything before an event arrives", async () => {
    const fake = await seedClient();
    const dispose = startInRoot();

    expect(vi.mocked(playNonStopAlarm)).not.toHaveBeenCalled();

    dispose();
  });
});
