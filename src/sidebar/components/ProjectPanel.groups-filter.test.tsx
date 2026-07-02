// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import {
  defaultGroupsConfig,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";

const projectPath = "C:\\Project";
const wg1Path = `${projectPath}\\.ac\\wg-1-dev-team`;
const wg2Path = `${projectPath}\\.ac\\wg-2-rust-team`;
const coordRowTestId = "replica.row.quick.wg-1-dev-team.dev-webpage-ui";

function groupsConfig(groups = [{ id: "frontend", name: "Frontend", regex: "^wg-1-" }]) {
  return {
    ...defaultGroupsConfig(),
    groups,
  };
}

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wg1Path,
        task: null,
        taskTitle: "Frontend work",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${wg1Path}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: `${wg1Path}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
      {
        name: "wg-2-rust-team",
        path: wg2Path,
        task: null,
        taskTitle: "Rust work",
        agents: [
          {
            name: "dev-rust",
            path: `${wg2Path}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function setupProjectTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.onInvoke("update_project_groups", (args) => args.config);
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function panelTarget<T extends Element>(root: ParentNode, testId: string): T {
  const element = root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

async function openCoordinatorMenu(root: ParentNode): Promise<void> {
  contextMenu(panelTarget(root, coordRowTestId));
  await waitFor(() => expect(document.querySelector(".session-context-menu")).not.toBeNull());
}

describe("ProjectPanel workgroup groups", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("ANDs the selected group with the existing visible regex filter", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig());
      workgroupGroupsStore.select(projectPath, { kind: "group", id: "frontend" });
      await projectStore.createAndLoad(projectPath);

      await waitFor(() => {
        expect(rendered.root.textContent).toContain("wg-1-dev-team");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });

      const toggle = panelTarget<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = panelTarget<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "rust-team");
      await waitFor(() => {
        expect(rendered.root.textContent).not.toContain("wg-1-dev-team");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("adds a coordinator workgroup to an existing group from the context menu", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${wg1Path}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig([{ id: "frontend", name: "Frontend", regex: "(?!)" }])
      );
      fake.clearCalls();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      click(target(`replica.wg-1-dev-team.groups.frontend`));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          groups: [
            {
              id: "frontend",
              regex: exactGroupRegexForWorkgroup("wg-1-dev-team"),
            },
          ],
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the context menu open and shows an error when inline group creation fails", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    fake.resolve("get_project_groups", groupsConfig([]));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.ensureLoaded(projectPath);
      fake.reject("update_project_groups", "groups.toml changed on disk");
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      click(target("replica.wg-1-dev-team.groups.create"));
      input(target<HTMLInputElement>("replica.groups.create.input"), "Frontend");
      click(target("replica.groups.create.save"));

      await waitFor(() => {
        expect(document.querySelector(".session-context-menu")).not.toBeNull();
        expect(target("replica.groups.error").textContent).toContain("groups.toml changed on disk");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
