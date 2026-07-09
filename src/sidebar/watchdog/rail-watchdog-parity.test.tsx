// @vitest-environment jsdom
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup, NonStopGroupConfig, Session } from "../../shared/types";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import WorkgroupGroupRail from "../components/WorkgroupGroupRail";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { defaultGroupsConfig, defaultNonStop, workgroupGroupsStore } from "../stores/workgroup-groups";
import { buildSnapshot } from "./non-stop-watchdog-client";

const projectPath = "C:\\Project";
const wgNames = ["wg-1-dev-team", "wg-2-dev-team", "wg-3-dev-team"] as const;

function wgPath(name: string): string {
  return `${projectPath}\\.ac\\${name}`;
}

function agentPath(name: string): string {
  return `${wgPath(name)}\\__agent_dev-webpage-ui`;
}

function wgDiscovery() {
  return discovery({
    workgroups: wgNames.map((name): AcWorkgroup => ({
      name,
      path: wgPath(name),
      task: null,
      taskTitle: null,
      agents: [{ name: "dev-webpage-ui", path: agentPath(name), repoPaths: [], isCoordinator: true }],
    })),
  });
}

function replicaSession(wgName: string, overrides: Partial<Session> = {}): Session {
  return session({
    id: `s-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: agentPath(wgName),
    status: "running",
    ...overrides,
  });
}

const activeNonStop = (): NonStopGroupConfig => ({
  ...defaultNonStop(),
  show: true,
  regex: "^wg-",
  telegram: { enabled: true, botId: null },
});

async function seed(): Promise<() => void> {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", wgDiscovery());
  fake.resolve("get_project_groups", { ...defaultGroupsConfig(), nonStop: activeNonStop() });
  const restore = __setTransportForTests(fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await workgroupGroupsStore.ensureLoaded(projectPath);
  return restore;
}

function renderRail(): () => void {
  const root = document.createElement("div");
  document.body.appendChild(root);
  const dispose = render(() => <WorkgroupGroupRail projects={projectStore.projects} />, root);
  return () => {
    dispose();
    root.remove();
  };
}

function nonStopButton(): HTMLElement {
  const button = document.querySelector<HTMLElement>('[data-ac-testid="workgroupGroups.button.nonstop"]');
  if (!button) throw new Error("Missing non-stop rail button");
  return button;
}

describe("#882 rail/watchdog working parity", () => {
  let restoreTransport: (() => void) | null = null;
  let cleanupRender: (() => void) | null = null;

  beforeEach(() => {
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  afterEach(() => {
    cleanupRender?.();
    cleanupRender = null;
    restoreTransport?.();
    restoreTransport = null;
    resetUiStoresForTests();
    workgroupGroupsStore.resetForTests();
  });

  it.each([
    {
      name: "all live working",
      sessions: [
        replicaSession("wg-1-dev-team"),
        replicaSession("wg-2-dev-team", { status: "active" }),
        replicaSession("wg-3-dev-team"),
      ],
      expectWorking: 3,
      expectDisparity: false,
      expectNotWorking: [],
    },
    {
      name: "absent, exited, and idle are not working",
      sessions: [
        replicaSession("wg-2-dev-team", { status: { exited: 0 } }),
        replicaSession("wg-3-dev-team", { status: "idle" }),
      ],
      expectWorking: 0,
      expectDisparity: true,
      expectNotWorking: ["wg-1-dev-team", "wg-2-dev-team", "wg-3-dev-team"],
    },
    {
      name: "waiting and pending are not working while active is working",
      sessions: [
        replicaSession("wg-1-dev-team", { waitingForInput: true }),
        replicaSession("wg-2-dev-team", { pendingReview: true }),
        replicaSession("wg-3-dev-team", { status: "active" }),
      ],
      expectWorking: 1,
      expectDisparity: true,
      expectNotWorking: ["wg-1-dev-team", "wg-2-dev-team"],
    },
    {
      name: "running plus pendingReview is not working (#882 AC)",
      sessions: [
        replicaSession("wg-1-dev-team", { pendingReview: true }),
        replicaSession("wg-2-dev-team", { pendingReview: true }),
        replicaSession("wg-3-dev-team", { pendingReview: true }),
      ],
      expectWorking: 0,
      expectDisparity: true,
      expectNotWorking: ["wg-1-dev-team", "wg-2-dev-team", "wg-3-dev-team"],
    },
  ])("$name", async (row) => {
    restoreTransport = await seed();
    sessionsStore.setSessions(row.sessions);
    cleanupRender = renderRail();

    await waitFor(() => expect(nonStopButton().textContent).toContain(`${row.expectWorking}/3`));
    const counter = nonStopButton().textContent?.match(/\d+\/\d+/)?.[0];
    const snap = buildSnapshot();

    expect(snap).toHaveLength(1);
    expect(counter).toBe(`${snap[0].working}/${snap[0].total}`);
    expect(snap[0].working).toBe(row.expectWorking);
    expect(snap[0].disparity).toBe(row.expectDisparity);
    expect(snap[0].notWorkingWorkgroups).toEqual(row.expectNotWorking);
  });
});
