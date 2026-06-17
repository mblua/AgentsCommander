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
});
