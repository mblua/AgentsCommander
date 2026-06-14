// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import type { AcLoopSummary } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
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

function enabledLoop(): AcLoopSummary {
  return { ...disabledLoop(), enabled: true };
}

function discoveryWithLoop(loop: AcLoopSummary) {
  return discovery({
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
    loops: [loop],
  });
}

function setupProject(fake: FakeTransport): void {
  fake.resolve("new_project", {
    path: projectPath,
    registered: true,
    created: false,
  });
  fake.resolve("discover_project", discoveryWithLoop(disabledLoop()));
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

  it("refreshes disabled loop row state and toggle text after Enable succeeds", async () => {
    const fake = new FakeTransport();
    let discoverCalls = 0;
    let releaseStaleReload: () => void = () => {};
    const staleReload = new Promise<void>((resolve) => {
      releaseStaleReload = resolve;
    });

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", async () => {
      discoverCalls += 1;
      if (discoverCalls === 1) return discoveryWithLoop(disabledLoop());
      if (discoverCalls === 2) {
        await staleReload;
        return discoveryWithLoop(disabledLoop());
      }
      return discoveryWithLoop(enabledLoop());
    });
    fake.onInvoke("toggle_loop", (args) => {
      expect(args).toEqual({
        projectPath,
        id: "weekday-standup",
        enabled: true,
      });
      return {
        summary: enabledLoop(),
        promptBody: "Short preview",
      };
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const loopId = automationIdPart("weekday-standup");
      const rowSelector = `[data-ac-testid="loop.row.${projectId}.${loopId}"]`;
      const toggleSelector = `[data-ac-testid="loop.action.toggle.${projectId}.${loopId}"]`;

      await waitFor(() => {
        expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
          "loop-disabled",
        );
      });

      const staleReloadPromise = projectStore.reloadProject(projectPath);
      await waitFor(() => expect(discoverCalls).toBe(2));

      const row = rendered.root.querySelector(rowSelector);
      if (!(row instanceof HTMLElement)) {
        throw new Error("Loop row not found");
      }
      contextMenu(row);

      await waitFor(() => {
        expect(document.body.querySelector(toggleSelector)?.textContent?.trim()).toBe("Enable");
      });

      const enableAction = document.body.querySelector(toggleSelector);
      if (!(enableAction instanceof HTMLButtonElement)) {
        throw new Error("Loop toggle action not found");
      }
      click(enableAction);

      await waitFor(() => expect(fake.callsFor("toggle_loop")).toHaveLength(1));
      await Promise.resolve();
      releaseStaleReload();
      await staleReloadPromise;

      await waitFor(() => {
        expect(discoverCalls).toBeGreaterThanOrEqual(3);
        expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
          "enabled",
        );
      });

      const refreshedRow = rendered.root.querySelector(rowSelector);
      if (!(refreshedRow instanceof HTMLElement)) {
        throw new Error("Refreshed Loop row not found");
      }
      contextMenu(refreshedRow);

      await waitFor(() => {
        expect(document.body.querySelector(toggleSelector)?.textContent?.trim()).toBe("Disable");
      });
    } finally {
      rendered.cleanup();
    }
  });
});
