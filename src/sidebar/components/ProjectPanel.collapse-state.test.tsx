// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { defaultGroupsConfig } from "../stores/workgroup-groups";
import type { AcDiscoveryResult, AcLoopSummary } from "../../shared/types";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;

function loop(overrides: Partial<AcLoopSummary> = {}): AcLoopSummary {
  return {
    id: "loop-standup",
    name: "Weekday standup",
    enabled: true,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: "wg-2-dev-team",
    promptPreview: "scheduled run",
    busyCoordinator: "skip",
    path: `${projectPath}\\.ac\\_loop_standup`,
    configPath: `${projectPath}\\.ac\\_loop_standup\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
    ...overrides,
  };
}

function projectDiscovery(taskTitle: string): AcDiscoveryResult {
  return discovery({
    agents: [
      {
        name: "AgentsCommander_ac/__agent_dev-webpage-ui",
        path: `${projectPath}\\.ac\\_agent_dev-webpage-ui`,
        roleExists: true,
      },
    ],
    teams: [
      {
        name: "frontend-team",
        agents: ["AgentsCommander_ac/__agent_dev-webpage-ui"],
        coordinator: "AgentsCommander_ac/__agent_dev-webpage-ui",
      },
    ],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle,
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${workgroupPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: `${workgroupPath}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
    loops: [loop()],
  });
}

function headerByName(root: ParentNode, name: string): HTMLElement {
  const headers = Array.from(root.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible"));
  const header = headers.find((candidate) =>
    candidate.querySelector(".ac-wg-name")?.textContent?.trim() === name
  );
  if (!header) throw new Error(`Header not found: ${name}`);
  return header;
}

function teamHeaderByName(root: ParentNode, name: string): HTMLElement {
  const headers = Array.from(root.querySelectorAll<HTMLElement>(".ac-team-header"));
  const header = headers.find((candidate) =>
    candidate.querySelector(".ac-team-name")?.textContent?.trim() === name
  );
  if (!header) throw new Error(`Team header not found: ${name}`);
  return header;
}

function headerCollapsed(header: HTMLElement): boolean {
  const chevron = header.querySelector(".ac-discovery-chevron");
  if (!chevron) throw new Error("Header chevron not found");
  return chevron.classList.contains("collapsed");
}

describe("ProjectPanel collapse state", () => {
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

  it("preserves Workgroups and workgroup subgroup collapse across project refresh", async () => {
    const fake = new FakeTransport();
    let discoverCount = 0;
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () => {
      discoverCount += 1;
      return projectDiscovery(discoverCount === 1 ? "Initial task title" : "Reloaded task title");
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-rust"));

      click(headerByName(rendered.root, "wg-2-dev-team"));
      await waitFor(() => expect(headerCollapsed(headerByName(rendered.root, "wg-2-dev-team"))).toBe(true));

      click(headerByName(rendered.root, "Workgroups"));
      await waitFor(() => expect(headerCollapsed(headerByName(rendered.root, "Workgroups"))).toBe(true));

      const reloaded = await projectStore.reloadProjectIfLoaded("c:/project/");
      expect(reloaded).toBe(true);
      await waitFor(() => expect(fake.callsFor("discover_project")).toHaveLength(2));
      await waitFor(() => expect(headerCollapsed(headerByName(rendered.root, "Workgroups"))).toBe(true));

      click(headerByName(rendered.root, "Workgroups"));
      await waitFor(() => expect(headerCollapsed(headerByName(rendered.root, "Workgroups"))).toBe(false));

      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Reloaded task title");
        expect(headerCollapsed(headerByName(rendered.root, "wg-2-dev-team"))).toBe(true);
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("preserves the other project sections and team expansion across reload", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery("Stable task title"));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("Weekday standup"));

      click(headerByName(rendered.root, "Loops"));
      click(headerByName(rendered.root, "Agents"));
      click(teamHeaderByName(rendered.root, "frontend-team"));

      await projectStore.reloadProject(projectPath);
      await waitFor(() => expect(fake.callsFor("discover_project")).toHaveLength(2));
      await waitFor(() => {
        expect(headerCollapsed(headerByName(rendered.root, "Loops"))).toBe(true);
        expect(headerCollapsed(headerByName(rendered.root, "Agents"))).toBe(true);
        expect(headerCollapsed(teamHeaderByName(rendered.root, "frontend-team"))).toBe(false);
        expect(rendered.root.textContent).toContain("dev-webpage-ui@AgentsCommander_ac");
      });

      click(headerByName(rendered.root, "Teams"));
      await projectStore.reloadProject(projectPath);
      await waitFor(() => expect(fake.callsFor("discover_project")).toHaveLength(3));
      await waitFor(() => expect(headerCollapsed(headerByName(rendered.root, "Teams"))).toBe(true));
    } finally {
      rendered.cleanup();
    }
  });

  it("preserves sub-section collapse when a rail selection collapses another project", async () => {
    // Cross-project case: owner A is selected from the rail, other
    // project B gets its project-level collapsed by collapseAllExceptKnown,
    // but B's "Workgroups" sub-section collapse choice must survive (sub-
    // sections stay in ProjectPanel's local collapsedByKey signal and are NOT
    // touched by the rail selection). Two backslash Windows paths exercise the
    // production collapse-key normalization.
    const otherProjectPath = "C:\\ProjectB";

    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPath) return projectDiscovery("Stable task title");
      if (path === otherProjectPath) {
        // Minimal discovery for B: a single workgroup so the project renders.
        return discovery({
          workgroups: [
            {
              name: "wg-1-dev-team",
              path: `${otherProjectPath}\\.ac\\wg-1-dev-team`,
              task: null,
              taskTitle: null,
              agents: [
                {
                  name: "dev-webpage-ui",
                  path: `${otherProjectPath}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
                  repoPaths: [],
                  isCoordinator: true,
                },
              ],
            },
          ],
        });
      }
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", { ...defaultGroupsConfig() });

    const rendered = renderWithFakeTransport(
      () => (
        <div class="sidebar-body">
          <WorkgroupGroupRail projects={projectStore.projects} />
          <div class="sidebar-scrollable">
            <ProjectPanel />
          </div>
        </div>
      ),
      fake
    );
    try {
      await projectStore.createAndLoad(projectPath);
      await projectStore.createAndLoad(otherProjectPath);
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Project: Project");
        expect(rendered.root.textContent).toContain("Project: ProjectB");
      });

      // Collapse B's "Workgroups" sub-section header before triggering auto-focus.
      const projectBHeader = Array.from(
        rendered.root.querySelectorAll<HTMLElement>(".project-header")
      ).find((h) => h.getAttribute("title") === otherProjectPath);
      if (!projectBHeader) throw new Error("ProjectB header not found");
      const projectBPanel = projectBHeader.closest(".project-panel");
      const workgroupsHeaderB = Array.from(
        projectBPanel!.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible")
      ).find((h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === "Workgroups");
      if (!workgroupsHeaderB) throw new Error("Workgroups sub-header not found in ProjectB");
      click(workgroupsHeaderB);
      await waitFor(() =>
        expect(workgroupsHeaderB.querySelector(".ac-discovery-chevron")?.classList.contains("collapsed")).toBe(true)
      );

      // Click a rail group button in ProjectA's section (owner).
      const railSectionA = document.querySelector<HTMLElement>('[data-ac-testid="workgroupGroups.rail.Project"]');
      if (!railSectionA) throw new Error("rail section for Project not found");
      const allButton = railSectionA.querySelector<HTMLElement>('[data-ac-testid="workgroupGroups.button.all"]');
      if (!allButton) throw new Error("All button not found in Project rail section");
      click(allButton);

      // B's project-level chevron is collapsed by the rail selection...
      await waitFor(() => {
        const chev = projectBHeader.querySelector(".ac-discovery-chevron");
        expect(chev?.classList.contains("collapsed")).toBe(true);
      });
      // ...but B's "Workgroups" sub-section collapse choice survives.
      expect(workgroupsHeaderB.querySelector(".ac-discovery-chevron")?.classList.contains("collapsed")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  // #1351 - the coordinator strip is now a real section: header, count, collapse.
  it("renders the Orchestrators header with a count and collapses its rows", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery("Stable task title"));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      const quickRow = '[data-ac-testid="replica.row.quick.wg-2-dev-team.dev-webpage-ui"]';
      await waitFor(() => expect(rendered.root.querySelector(quickRow)).not.toBeNull());

      const header = headerByName(rendered.root, "Orchestrators");
      expect(header.querySelector(".ac-team-count")?.textContent?.trim()).toBe("1");
      expect(headerCollapsed(header)).toBe(false);

      click(header);
      await waitFor(() => {
        expect(headerCollapsed(headerByName(rendered.root, "Orchestrators"))).toBe(true);
        expect(rendered.root.querySelector(quickRow)).toBeNull();
      });
    } finally {
      rendered.cleanup();
    }
  });
});
