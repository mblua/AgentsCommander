// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type { BlockerReport } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  input,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-1-dev-team`;

function initialDiscovery() {
  return discovery({
    agents: [],
    teams: [
      {
        name: "dev-team",
        agents: ["architect"],
        coordinator: "architect",
      },
    ],
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Delete diagnostics",
        teamName: "dev-team",
        agents: [
          {
            name: "architect",
            path: `${workgroupPath}\\__agent_architect`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

const blockerReport: BlockerReport = {
  workgroup: "wg-1-dev-team",
  platform: "windows",
  diagnosticAvailable: true,
  rawOsError: "sharing violation",
  sessions: [],
  processes: [],
  liveSessions: [
    {
      sessionId: "session-1",
      agentName: "dev-webpage-ui",
      cwd: `${workgroupPath}\\__agent_dev-webpage-ui`,
    },
  ],
  externalProcesses: [
    {
      pid: 4242,
      name: "node.exe",
      cwd: projectPath,
      files: [`${workgroupPath}\\package.json`],
    },
  ],
};

function findButton(text: string): HTMLButtonElement {
  const button = Array.from(document.body.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === text
  );
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Button not found: ${text}`);
  }
  return button;
}

function findWorkgroupHeader(root: ParentNode): Element {
  const header = Array.from(root.querySelectorAll(".ac-wg-header")).find(
    (candidate) =>
      candidate.getAttribute("title") === workgroupPath &&
      candidate.textContent?.includes("wg-1-dev-team")
  );
  if (!header) {
    throw new Error("Workgroup header not found");
  }
  return header;
}

describe("ProjectPanel workgroup delete diagnostics workflow", () => {
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

  it("shows blocker diagnostics and retries delete with the same force flag", async () => {
    const fake = new FakeTransport();
    let deleteAttempts = 0;

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () =>
      deleteAttempts > 1 ? discovery({ teams: [], workgroups: [], agents: [] }) : initialDiscovery()
    );
    fake.onInvoke("delete_workgroup", () => {
      deleteAttempts += 1;
      if (deleteAttempts === 1) {
        throw `BLOCKERS:${JSON.stringify(blockerReport)}`;
      }
      return undefined;
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("wg-1-dev-team"));

      contextMenu(findWorkgroupHeader(rendered.root));
      await waitFor(() => expect(document.body.textContent).toContain("Delete Workgroup"));
      click(findButton("Delete Workgroup"));

      await waitFor(() => expect(document.body.textContent).toContain("This action cannot be undone"));
      click(findButton("Delete"));

      await waitFor(() => expect(fake.callsFor("delete_workgroup")).toHaveLength(1));
      expect(fake.callsFor("delete_workgroup")[0].args).toEqual({
        projectPath,
        workgroupName: "wg-1-dev-team",
        force: false,
      });

      await waitFor(() => {
        expect(document.body.textContent).toContain("dev-webpage-ui");
        expect(document.body.textContent).toContain("node.exe");
      });

      click(findButton("Retry"));

      await waitFor(() => expect(fake.callsFor("delete_workgroup")).toHaveLength(2));
      expect(fake.callsFor("delete_workgroup")[1].args).toEqual({
        projectPath,
        workgroupName: "wg-1-dev-team",
        force: false,
      });
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThan(1));
    } finally {
      rendered.cleanup();
    }
  });

  it("requires confirmation and retries dirty repo deletes with force", async () => {
    const fake = new FakeTransport();
    let deleteAttempts = 0;

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () =>
      deleteAttempts > 1 ? discovery({ teams: [], workgroups: [], agents: [] }) : initialDiscovery()
    );
    fake.onInvoke("delete_workgroup", () => {
      deleteAttempts += 1;
      if (deleteAttempts === 1) {
        throw "DIRTY_REPOS:repo has uncommitted changes";
      }
      return undefined;
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("wg-1-dev-team"));

      contextMenu(findWorkgroupHeader(rendered.root));
      await waitFor(() => expect(document.body.textContent).toContain("Delete Workgroup"));
      click(findButton("Delete Workgroup"));

      await waitFor(() => expect(document.body.textContent).toContain("This action cannot be undone"));
      click(findButton("Delete"));

      await waitFor(() => expect(document.body.textContent).toContain("repo has uncommitted changes"));

      const confirmInput = document.body.querySelector(".new-agent-input");
      if (!(confirmInput instanceof HTMLInputElement)) {
        throw new Error("Dirty repo confirmation input not found");
      }
      input(confirmInput, "wg-1-dev-team");
      click(findButton("Delete"));

      await waitFor(() => expect(fake.callsFor("delete_workgroup")).toHaveLength(2));
      expect(fake.callsFor("delete_workgroup")[1].args).toEqual({
        projectPath,
        workgroupName: "wg-1-dev-team",
        force: true,
      });
    } finally {
      rendered.cleanup();
    }
  });
});
