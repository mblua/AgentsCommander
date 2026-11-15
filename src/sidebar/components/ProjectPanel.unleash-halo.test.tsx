// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";

// #4752 P06: the `unleashed` class on a room replica tile drives the halo.

const projectPath = "C:\\Project";
const wgPath = `${projectPath}\\.ac\\wg-1-dev-team`;
const wildPath = `${wgPath}\\__agent_dev-rust`;
const tamePath = `${wgPath}\\__agent_dev-web`;
const itemTestId = "replica.wild.menu.unleash";
const rowId = (name: string) => `replica.row.workgroups.wg-1-dev-team.${name}`;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const byTestId = (id: string) => document.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const tile = (name: string) => byTestId(rowId(name))!;

const agent = (name: string, path: string, unleashed: boolean) => ({
  name,
  path,
  identityPath: `../../_agent_${name}`,
  repoPaths: [],
  isCoordinator: false,
  unleashed,
});

describe("ProjectPanel unleashed halo class (#4752)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  async function setupPanel() {
    const fake = new FakeTransport();
    const result = discovery({
      workgroups: [
        {
          name: "wg-1-dev-team",
          path: wgPath,
          task: null,
          taskTitle: "Clean",
          agents: [agent("dev-rust", wildPath, true), agent("dev-web", tamePath, false)],
        },
      ],
    });
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", { ...result, teams: [] });
    fake.resolve("get_settings", baseSettings());
    fake.resolve("set_replica_unleashed", undefined);
    fake.onInvoke("restart_session", ({ id }) => session({ id: String(id) }));
    fake.onInvoke("task_get_snapshot_at", ({ workgroupRoot }) => ({
      workgroupRoot: String(workgroupRoot), task: null, taskTitle: "Clean", description: "",
      status: null, revision: "topic:0", statusRecord: null, tailIncomplete: false,
    }));
    sessionsStore.setSessions([
      session({ id: "wild", name: "wg-1-dev-team/dev-rust",
        workingDirectory: wildPath, status: "running" }),
      session({ id: "tame", name: "wg-1-dev-team/dev-web",
        workingDirectory: tamePath, status: "running" }),
    ]);
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(byTestId(rowId("dev-web"))).not.toBeNull());
    return fake;
  }

  it("marks only the unleashed replica tile", async () => {
    await setupPanel();
    expect(tile("dev-rust").classList.contains("replica-item")).toBe(true);
    expect(tile("dev-rust").classList.contains("unleashed")).toBe(true);
    expect(tile("dev-web").classList.contains("unleashed")).toBe(false);
  });

  it("drops the class after a successful Leash, without a discovery refresh", async () => {
    const fake = await setupPanel();
    const discoveries = fake.callsFor("discover_project").length;
    contextMenu(tile("dev-rust"));
    await waitFor(() => expect(byTestId(itemTestId)).not.toBeNull());
    await flush();
    click(byTestId(itemTestId)!);
    await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
    await waitFor(() => expect(tile("dev-rust").classList.contains("unleashed")).toBe(false));
    expect(fake.callsFor("set_replica_unleashed").map((c) => c.args)).toEqual([
      { replicaPath: wildPath, unleashed: false },
    ]);
    expect(fake.callsFor("discover_project")).toHaveLength(discoveries);
    expect(tile("dev-web").classList.contains("unleashed")).toBe(false);
  });
});
