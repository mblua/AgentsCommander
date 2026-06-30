// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
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
});
