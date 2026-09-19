// @vitest-environment jsdom
//
// #2202 AC 4, the ordering half, end to end through the rendered sidebar rather
// than through markActivity alone: a repo leaving `ci: "running"` must reach
// `lastActivityBySessionId` and make `naturalCoordinatorItems` re-sort, so the
// room whose CI just finished floats to the top of the coordinator rows.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { remoteActivityStore } from "../stores/remote-activity";
import type { CiState } from "../../shared/types";

const projectPath = "C:\\Project";
const wg1Path = `${projectPath}\\.ac\\wg-1-dev-team`;
const wg2Path = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordAPath = `${wg1Path}\\__agent_coord-a`;
const coordBPath = `${wg2Path}\\__agent_coord-b`;
// Only the OLDER room (wg-2) carries a CI repo; it is the one that must float.
const ciRepo = `${wg2Path}\\repo-AgentsCommander`;

const SESSION_A = "session-coord-a";
const SESSION_B = "session-coord-b";
const rowA = "replica.row.quick.wg-1-dev-team.coord-a";
const rowB = "replica.row.quick.wg-2-dev-team.coord-b";

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wg1Path,
        task: null,
        taskTitle: null,
        agents: [{ name: "coord-a", path: coordAPath, repoPaths: [], isCoordinator: true }],
      },
      {
        name: "wg-2-dev-team",
        path: wg2Path,
        task: null,
        taskTitle: null,
        agents: [{ name: "coord-b", path: coordBPath, repoPaths: [ciRepo], isCoordinator: true }],
      },
    ],
  });
}

function liveSessions() {
  return [
    session({
      id: SESSION_A,
      name: "wg-1-dev-team/coord-a",
      workingDirectory: coordAPath,
      status: "idle",
      isCoordinator: true,
    }),
    session({
      id: SESSION_B,
      name: "wg-2-dev-team/coord-b",
      workingDirectory: coordBPath,
      status: "idle",
      isCoordinator: true,
    }),
  ];
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", liveSessions());
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

/** The store replaces the WHOLE map per update, so all repos go in ONE call. */
function publishCi(entries: [string, CiState][]): void {
  remoteActivityStore.applyRemoteActivityUpdate({
    repoPaths: entries.map(([repoPath]) => repoPath),
    ciStates: entries.map(([, ci]) => ci),
    stalenessStates: entries.map(() => "current" as const),
    behindBy: entries.map(() => null),
  });
}

function quickOrder(rendered: { root: HTMLDivElement }): string[] {
  return Array.from(
    rendered.root.querySelectorAll<HTMLElement>('[data-ac-testid^="replica.row.quick."]')
  ).map((el) => el.getAttribute("data-ac-testid")!);
}

async function mount(sortByActivity: boolean) {
  const fake = new FakeTransport();
  setupTransport(fake);
  const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
  await waitFor(() => expect(quickOrder(rendered).sort()).toEqual([rowA, rowB]));

  // wg-2 is the OLDER room, so it renders second while wg-1 holds the newer stamp.
  sessionsStore.markActivity(SESSION_B);
  sessionsStore.markActivity(SESSION_A);
  publishCi([[ciRepo, "running"]]);
  sessionsStore.setCoordSortByActivity(sortByActivity);
  await waitFor(() => expect(quickOrder(rendered)).toEqual([rowA, rowB]));
  return rendered;
}

describe("#2202 CI stop floats the room under sort-by-activity", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("re-sorts the coordinator rows when the older room's CI finishes", async () => {
    const rendered = await mount(true);
    try {
      publishCi([[ciRepo, "idle"]]);
      await waitFor(() => expect(quickOrder(rendered)).toEqual([rowB, rowA]));
    } finally {
      rendered.cleanup();
    }
  });

  it("control: the same update leaves the order alone with coordSortByActivity off", async () => {
    const rendered = await mount(false);
    try {
      publishCi([[ciRepo, "idle"]]);
      // The stamp still lands; the order does not follow it.
      await waitFor(() => expect(sessionsStore.lastActivityBySessionId[SESSION_B]).toBeTypeOf("number"));
      expect(quickOrder(rendered)).toEqual([rowA, rowB]);
    } finally {
      rendered.cleanup();
    }
  });
});
