// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
  session,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { AcLoopSummary } from "../../shared/types";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;

function projectDiscovery() {
  return discovery({
    agents: [],
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
        taskTitle: "Sidebar regex filter",
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
  });
}

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

function discoveryWithLoops(loops: AcLoopSummary[]) {
  return { ...projectDiscovery(), loops };
}

function twoWorkgroupDiscovery() {
  return discovery({
    agents: [],
    teams: [],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Sidebar regex filter",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${workgroupPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
      {
        name: "wg-3-ops-team",
        path: `${projectPath}\\.ac\\wg-3-ops-team`,
        task: null,
        taskTitle: "Ops rotation",
        agents: [
          {
            name: "ops-lead",
            path: `${projectPath}\\.ac\\wg-3-ops-team\\__agent_ops-lead`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function findByTestId<T extends Element>(root: ParentNode, testId: string): T {
  const element = root.querySelector(`[data-ac-testid="${testId}"]`);
  if (!element) {
    throw new Error(`Element not found: ${testId}`);
  }
  return element as T;
}

describe("ProjectPanel regex filter", () => {
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

  it("opens inline, filters presentation, reports invalid regex, and clears with Escape", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));
      expect(rendered.root.textContent).toContain("dev-rust");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);

      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));
      await waitFor(() => expect(document.activeElement).toBe(filterInput));

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      input(filterInput, "(");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Invalid regex");
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust");
      });

      filterInput.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await waitFor(() => {
        expect(filterInput.value).toBe("");
        expect(rendered.root.textContent).not.toContain("Invalid regex");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("matches visible team project labels and quick-access running-peer badges", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.resolve("discover_project", projectDiscovery());

    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-2-dev-team/dev-webpage-ui",
        workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
        status: "running",
      }),
      session({
        id: "peer-session",
        name: "wg-2-dev-team/dev-rust",
        workingDirectory: `${workgroupPath}\\__agent_dev-rust`,
        status: "running",
      }),
    ]);
    sessionsStore.setActiveId("coord-session");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("frontend-team"));
      const teamHeader = Array.from(rendered.root.querySelectorAll(".ac-team-header")).find((header) =>
        header.textContent?.includes("frontend-team")
      );
      if (!teamHeader) {
        throw new Error("Team header not found");
      }
      click(teamHeader);

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "AgentsCommander_ac");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui@AgentsCommander_ac");
        expect(rendered.root.textContent).toContain("frontend-team");
      });

      input(filterInput, "RUNNING");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust RUNNING");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("filters loop rows and treats the Loops section label as matchable", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve(
      "discover_project",
      discoveryWithLoops([
        loop({ id: "loop-standup", name: "Weekday standup", promptPreview: "morning sync" }),
        loop({
          id: "loop-backup",
          name: "Nightly backup",
          expr: "0 2 * * *",
          promptPreview: "archive job",
          path: `${projectPath}\\.ac\\_loop_backup`,
          configPath: `${projectPath}\\.ac\\_loop_backup\\config.toml`,
        }),
      ])
    );

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("Weekday standup"));
      expect(rendered.root.textContent).toContain("Nightly backup");

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "standup");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Weekday standup");
        expect(rendered.root.textContent).not.toContain("Nightly backup");
      });

      // The section label itself is matchable -> every loop shows again.
      input(filterInput, "Loops");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("Weekday standup");
        expect(rendered.root.textContent).toContain("Nightly backup");
      });

      // No match anywhere -> loop rows hide cleanly with no empty-state hint.
      input(filterInput, "zzz-no-such-loop");
      await waitFor(() => {
        expect(rendered.root.textContent).not.toContain("Weekday standup");
        expect(rendered.root.textContent).not.toContain("Nightly backup");
        expect(rendered.root.textContent).not.toContain("No loops");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("updates the Workgroups section count to reflect filtered matches", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", twoWorkgroupDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("wg-3-ops-team"));

      const workgroupsCount = () => {
        const header = Array.from(rendered.root.querySelectorAll(".ac-wg-header")).find(
          (h) => h.querySelector(".ac-wg-name")?.textContent === "Workgroups"
        );
        return header?.querySelector(".ac-team-count")?.textContent ?? null;
      };

      await waitFor(() => expect(workgroupsCount()).toBe("2"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "ops-team");
      await waitFor(() => {
        expect(workgroupsCount()).toBe("1");
        expect(rendered.root.textContent).toContain("wg-3-ops-team");
        expect(rendered.root.textContent).not.toContain("wg-2-dev-team");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("closes and clears the active filter when the magnifier is toggled", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
      });

      // Toggling the magnifier closes the field AND drops the filter so no
      // active-but-hidden filter survives.
      click(toggle);
      await waitFor(() => {
        expect(toggle.getAttribute("aria-expanded")).toBe("false");
        expect(filterInput.value).toBe("");
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).toContain("dev-rust");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
