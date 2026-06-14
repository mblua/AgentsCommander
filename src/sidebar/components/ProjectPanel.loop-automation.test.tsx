// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type { AcLoopSummary } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { automationIdPart } from "./replica-repo-badges";

const projectPath = "C:\\Project";
const workgroupName = "wg-1-dev-team";
const workgroupPath = `${projectPath}\\.ac\\${workgroupName}`;

function disabledLoop(): AcLoopSummary {
  return {
    id: "weekday-standup",
    name: "Weekday standup",
    enabled: false,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: workgroupName,
    promptPreview: "Short preview",
    busyCoordinator: "skip",
    path: `${projectPath}\\.ac\\_loop_weekday-standup`,
    configPath: `${projectPath}\\.ac\\_loop_weekday-standup\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
  };
}

function setupProject(fake: FakeTransport): void {
  fake.resolve("new_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve(
    "discover_project",
    discovery({
      teams: [
        {
          name: "dev-team",
          agents: ["architect"],
          coordinator: "architect",
        },
      ],
      workgroups: [
        {
          name: workgroupName,
          path: workgroupPath,
          task: null,
          taskTitle: "Loop automation",
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
      loops: [disabledLoop()],
    }),
  );
}

describe("ProjectPanel loop automation hooks", () => {
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

  it("keeps disabled loop rows context-menuable for automation", async () => {
    const fake = new FakeTransport();
    setupProject(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const loopId = automationIdPart("weekday-standup");
      await waitFor(() => {
        const candidate = rendered.root.querySelector(
          `[data-ac-testid="loop.row.${projectId}.${loopId}"]`,
        );
        expect(candidate).toBeTruthy();
      });
      const row = rendered.root.querySelector(
        `[data-ac-testid="loop.row.${projectId}.${loopId}"]`,
      );
      if (!(row instanceof HTMLElement)) {
        throw new Error("Loop row not found");
      }

      expect(row.getAttribute("data-ac-state")).toBe("loop-disabled");

      contextMenu(row);

      await waitFor(() => {
        const toggle = document.body.querySelector(
          `[data-ac-testid="loop.action.toggle.${projectId}.${loopId}"]`,
        );
        const deleteAction = document.body.querySelector(
          `[data-ac-testid="loop.action.delete.${projectId}.${loopId}"]`,
        );
        expect(toggle?.textContent?.trim()).toBe("Enable");
        expect(deleteAction?.textContent?.trim()).toBe("Delete Loop");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
