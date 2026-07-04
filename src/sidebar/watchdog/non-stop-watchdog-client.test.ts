// @vitest-environment jsdom
//
// #777 Non-stop watchdog client. Tests the pure detection (buildSnapshot) against
// seeded stores: the disparity signal is computed with the exact rail code
// (workgroupIsWorking + projectStore.projects), so this proves counter parity and
// the onset/clear flip. The createEffect + 10s keepalive wiring in
// startNonStopWatchdogClient is audited by review (team-idle-watcher convention),
// not unit-mocked here.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { baseSettings, discovery, resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import { defaultGroupsConfig, defaultNonStop, workgroupGroupsStore } from "../stores/workgroup-groups";
import type { NonStopGroupConfig } from "../../shared/types";
import { buildSnapshot } from "./non-stop-watchdog-client";

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
