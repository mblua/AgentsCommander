// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

function dailyReportLoop(): AcLoopSummary {
  return {
    ...disabledLoop(),
    id: "daily-report",
    name: "Daily report",
    enabled: true,
    expr: "0 17 * * 1-5",
    path: `${projectPath}\\.ac\\_loop_daily-report`,
    configPath: `${projectPath}\\.ac\\_loop_daily-report\\config.toml`,
  };
}

function deferred<T = void>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function discoveryWithLoops(loops: AcLoopSummary[]) {
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
    loops,
  });
}

function discoveryWithLoop(loop: AcLoopSummary) {
  return discoveryWithLoops([loop]);
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

  it("opens selector-addressable delete confirmation and keeps the loop on cancel", async () => {
    const fake = new FakeTransport();
    setupProject(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const loopId = automationIdPart("weekday-standup");
      const rowSelector = `[data-ac-testid="loop.row.${projectId}.${loopId}"]`;
      const deleteActionSelector = `[data-ac-testid="loop.action.delete.${projectId}.${loopId}"]`;
      const cancelSelector = `[data-ac-testid="loop.delete.cancel.${projectId}.${loopId}"]`;

      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeTruthy());
      const row = rendered.root.querySelector(rowSelector);
      if (!(row instanceof HTMLElement)) {
        throw new Error("Loop row not found");
      }

      contextMenu(row);
      await waitFor(() => expect(document.body.querySelector(deleteActionSelector)).toBeTruthy());
      const deleteAction = document.body.querySelector(deleteActionSelector);
      if (!(deleteAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete action not found");
      }
      click(deleteAction);

      await waitFor(() => expect(document.body.querySelector(cancelSelector)).toBeTruthy());
      const cancelAction = document.body.querySelector(cancelSelector);
      if (!(cancelAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete cancel action not found");
      }
      click(cancelAction);

      expect(fake.callsFor("delete_loop")).toHaveLength(0);
      expect(rendered.root.querySelector(rowSelector)).toBeTruthy();
      expect(document.body.querySelector(cancelSelector)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("confirms loop delete through selectors and removes the visible loop", async () => {
    const fake = new FakeTransport();
    let deleted = false;

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () =>
      deleted ? discoveryWithLoops([]) : discoveryWithLoop(disabledLoop())
    );
    fake.onInvoke("delete_loop", (args) => {
      expect(args).toEqual({
        projectPath,
        id: "weekday-standup",
      });
      deleted = true;
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const loopId = automationIdPart("weekday-standup");
      const rowSelector = `[data-ac-testid="loop.row.${projectId}.${loopId}"]`;
      const deleteActionSelector = `[data-ac-testid="loop.action.delete.${projectId}.${loopId}"]`;
      const confirmSelector = `[data-ac-testid="loop.delete.confirm.${projectId}.${loopId}"]`;

      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeTruthy());
      const row = rendered.root.querySelector(rowSelector);
      if (!(row instanceof HTMLElement)) {
        throw new Error("Loop row not found");
      }

      contextMenu(row);
      await waitFor(() => expect(document.body.querySelector(deleteActionSelector)).toBeTruthy());
      const deleteAction = document.body.querySelector(deleteActionSelector);
      if (!(deleteAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete action not found");
      }
      click(deleteAction);

      await waitFor(() => expect(document.body.querySelector(confirmSelector)).toBeTruthy());
      const confirmAction = document.body.querySelector(confirmSelector);
      if (!(confirmAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete confirm action not found");
      }
      click(confirmAction);

      await waitFor(() => expect(fake.callsFor("delete_loop")).toHaveLength(1));
      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeNull());
      expect(document.body.querySelector(confirmSelector)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("blocks opening loop delete while an unrelated loop action is in progress", async () => {
    const fake = new FakeTransport();
    const runNow = deferred();
    const primaryLoop = disabledLoop();
    const unrelatedLoop = dailyReportLoop();

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () => discoveryWithLoops([primaryLoop, unrelatedLoop]));
    fake.onInvoke("run_loop_now", async (args) => {
      expect(args).toEqual({
        projectPath,
        id: primaryLoop.id,
      });
      await runNow.promise;
      return {
        summary: primaryLoop,
        promptBody: "Short preview",
      };
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const primaryLoopId = automationIdPart(primaryLoop.id);
      const unrelatedLoopId = automationIdPart(unrelatedLoop.id);
      const primaryRowSelector = `[data-ac-testid="loop.row.${projectId}.${primaryLoopId}"]`;
      const unrelatedRowSelector = `[data-ac-testid="loop.row.${projectId}.${unrelatedLoopId}"]`;
      const runSelector = `[data-ac-testid="loop.action.runNow.${projectId}.${primaryLoopId}"]`;
      const deleteActionSelector = `[data-ac-testid="loop.action.delete.${projectId}.${unrelatedLoopId}"]`;
      const deleteDialogSelector = `[data-ac-testid="loop.delete.dialog.${projectId}.${unrelatedLoopId}"]`;

      await waitFor(() => expect(rendered.root.querySelector(primaryRowSelector)).toBeTruthy());
      await waitFor(() => expect(rendered.root.querySelector(unrelatedRowSelector)).toBeTruthy());

      const primaryRow = rendered.root.querySelector(primaryRowSelector);
      if (!(primaryRow instanceof HTMLElement)) {
        throw new Error("Primary Loop row not found");
      }
      contextMenu(primaryRow);
      await waitFor(() => expect(document.body.querySelector(runSelector)).toBeTruthy());
      const runAction = document.body.querySelector(runSelector);
      if (!(runAction instanceof HTMLButtonElement)) {
        throw new Error("Loop run action not found");
      }
      click(runAction);
      await waitFor(() => expect(fake.callsFor("run_loop_now")).toHaveLength(1));

      const unrelatedRow = rendered.root.querySelector(unrelatedRowSelector);
      if (!(unrelatedRow instanceof HTMLElement)) {
        throw new Error("Unrelated Loop row not found");
      }
      contextMenu(unrelatedRow);
      await waitFor(() => expect(document.body.querySelector(deleteActionSelector)).toBeTruthy());
      const deleteAction = document.body.querySelector(deleteActionSelector);
      if (!(deleteAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete action not found");
      }

      expect(deleteAction.disabled).toBe(true);
      click(deleteAction);
      expect(document.body.querySelector(deleteDialogSelector)).toBeNull();

      runNow.resolve();
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2));
    } finally {
      runNow.resolve();
      rendered.cleanup();
    }
  });

  it("keeps loop delete cancel and Escape available during an unrelated loop action", async () => {
    const fake = new FakeTransport();
    const pendingRuns = [deferred(), deferred()];
    const primaryLoop = disabledLoop();
    const unrelatedLoop = dailyReportLoop();
    let runIndex = 0;

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () => discoveryWithLoops([primaryLoop, unrelatedLoop]));
    fake.onInvoke("run_loop_now", async (args) => {
      expect(args).toEqual({
        projectPath,
        id: unrelatedLoop.id,
      });
      const pendingRun = pendingRuns[runIndex];
      runIndex += 1;
      await pendingRun.promise;
      return {
        summary: unrelatedLoop,
        promptBody: "Daily preview",
      };
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    const projectId = automationIdPart(projectPath);
    const primaryLoopId = automationIdPart(primaryLoop.id);
    const unrelatedLoopId = automationIdPart(unrelatedLoop.id);
    const primaryRowSelector = `[data-ac-testid="loop.row.${projectId}.${primaryLoopId}"]`;
    const unrelatedRowSelector = `[data-ac-testid="loop.row.${projectId}.${unrelatedLoopId}"]`;
    const deleteActionSelector = `[data-ac-testid="loop.action.delete.${projectId}.${primaryLoopId}"]`;
    const runSelector = `[data-ac-testid="loop.action.runNow.${projectId}.${unrelatedLoopId}"]`;
    const dialogSelector = `[data-ac-testid="loop.delete.dialog.${projectId}.${primaryLoopId}"]`;
    const cancelSelector = `[data-ac-testid="loop.delete.cancel.${projectId}.${primaryLoopId}"]`;
    const confirmSelector = `[data-ac-testid="loop.delete.confirm.${projectId}.${primaryLoopId}"]`;

    const openDeleteDialog = async () => {
      const primaryRow = rendered.root.querySelector(primaryRowSelector);
      if (!(primaryRow instanceof HTMLElement)) {
        throw new Error("Primary Loop row not found");
      }
      contextMenu(primaryRow);
      await waitFor(() => {
        const deleteAction = document.body.querySelector(deleteActionSelector);
        expect(deleteAction instanceof HTMLButtonElement && !deleteAction.disabled).toBe(true);
      });
      const deleteAction = document.body.querySelector(deleteActionSelector);
      if (!(deleteAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete action not found");
      }
      click(deleteAction);
      await waitFor(() => expect(document.body.querySelector(dialogSelector)).toBeTruthy());
    };

    const startUnrelatedRun = async (expectedCalls: number) => {
      const unrelatedRow = rendered.root.querySelector(unrelatedRowSelector);
      if (!(unrelatedRow instanceof HTMLElement)) {
        throw new Error("Unrelated Loop row not found");
      }
      contextMenu(unrelatedRow);
      await waitFor(() => expect(document.body.querySelector(runSelector)).toBeTruthy());
      const runAction = document.body.querySelector(runSelector);
      if (!(runAction instanceof HTMLButtonElement)) {
        throw new Error("Loop run action not found");
      }
      click(runAction);
      await waitFor(() => expect(fake.callsFor("run_loop_now")).toHaveLength(expectedCalls));
    };

    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.querySelector(primaryRowSelector)).toBeTruthy());
      await waitFor(() => expect(rendered.root.querySelector(unrelatedRowSelector)).toBeTruthy());

      await openDeleteDialog();
      await startUnrelatedRun(1);

      const cancelAction = document.body.querySelector(cancelSelector);
      const confirmAction = document.body.querySelector(confirmSelector);
      if (!(cancelAction instanceof HTMLButtonElement) || !(confirmAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete modal actions not found");
      }
      expect(cancelAction.disabled).toBe(false);
      expect(confirmAction.disabled).toBe(true);
      click(cancelAction);
      await waitFor(() => expect(document.body.querySelector(dialogSelector)).toBeNull());
      expect(fake.callsFor("delete_loop")).toHaveLength(0);

      pendingRuns[0].resolve();
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(2));

      await openDeleteDialog();
      await startUnrelatedRun(2);
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await waitFor(() => expect(document.body.querySelector(dialogSelector)).toBeNull());
      expect(fake.callsFor("delete_loop")).toHaveLength(0);

      pendingRuns[1].resolve();
      await waitFor(() => expect(fake.callsFor("discover_project").length).toBeGreaterThanOrEqual(3));
    } finally {
      pendingRuns[0].resolve();
      pendingRuns[1].resolve();
      rendered.cleanup();
    }
  });

  it("keeps loop delete cancel and Escape guarded while the current delete is in progress", async () => {
    const fake = new FakeTransport();
    const deletePending = deferred();
    const primaryLoop = disabledLoop();
    const unrelatedLoop = dailyReportLoop();
    let deleted = false;

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", () =>
      deleted ? discoveryWithLoops([unrelatedLoop]) : discoveryWithLoops([primaryLoop, unrelatedLoop])
    );
    fake.onInvoke("delete_loop", async (args) => {
      expect(args).toEqual({
        projectPath,
        id: primaryLoop.id,
      });
      await deletePending.promise;
      deleted = true;
    });

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);

      const projectId = automationIdPart(projectPath);
      const primaryLoopId = automationIdPart(primaryLoop.id);
      const rowSelector = `[data-ac-testid="loop.row.${projectId}.${primaryLoopId}"]`;
      const deleteActionSelector = `[data-ac-testid="loop.action.delete.${projectId}.${primaryLoopId}"]`;
      const dialogSelector = `[data-ac-testid="loop.delete.dialog.${projectId}.${primaryLoopId}"]`;
      const cancelSelector = `[data-ac-testid="loop.delete.cancel.${projectId}.${primaryLoopId}"]`;
      const confirmSelector = `[data-ac-testid="loop.delete.confirm.${projectId}.${primaryLoopId}"]`;

      await waitFor(() => expect(rendered.root.querySelector(rowSelector)).toBeTruthy());
      const row = rendered.root.querySelector(rowSelector);
      if (!(row instanceof HTMLElement)) {
        throw new Error("Loop row not found");
      }

      contextMenu(row);
      await waitFor(() => expect(document.body.querySelector(deleteActionSelector)).toBeTruthy());
      const deleteAction = document.body.querySelector(deleteActionSelector);
      if (!(deleteAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete action not found");
      }
      click(deleteAction);

      await waitFor(() => expect(document.body.querySelector(confirmSelector)).toBeTruthy());
      const confirmAction = document.body.querySelector(confirmSelector);
      if (!(confirmAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete confirm action not found");
      }
      click(confirmAction);
      await waitFor(() => expect(fake.callsFor("delete_loop")).toHaveLength(1));

      const cancelAction = document.body.querySelector(cancelSelector);
      if (!(cancelAction instanceof HTMLButtonElement)) {
        throw new Error("Loop delete cancel action not found");
      }
      expect(cancelAction.disabled).toBe(true);
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(document.body.querySelector(dialogSelector)).toBeTruthy();
      click(cancelAction);
      expect(document.body.querySelector(dialogSelector)).toBeTruthy();

      deletePending.resolve();
      await waitFor(() => expect(document.body.querySelector(dialogSelector)).toBeNull());
    } finally {
      deletePending.resolve();
      rendered.cleanup();
    }
  });

  it("does not regress enabled loop row while a queued reload follows a stale response", async () => {
    const fake = new FakeTransport();
    let discoverCalls = 0;
    const staleReload = deferred<void>();
    const queuedReload = deferred<void>();

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", async () => {
      discoverCalls += 1;
      if (discoverCalls === 1) return discoveryWithLoop(disabledLoop());
      if (discoverCalls === 2) {
        await staleReload.promise;
        return discoveryWithLoop(disabledLoop());
      }
      if (discoverCalls === 3) {
        await queuedReload.promise;
        return discoveryWithLoop(enabledLoop());
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
      await waitFor(() => {
        expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
          "enabled",
        );
      });

      staleReload.resolve();
      await waitFor(() => expect(discoverCalls).toBe(3));

      expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
        "enabled",
      );

      queuedReload.resolve();
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

  it("keeps the enabled loop summary visible when the queued follow-up reload fails", async () => {
    const fake = new FakeTransport();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    let discoverCalls = 0;
    const staleReload = deferred<void>();
    const queuedReload = deferred<void>();

    fake.resolve("new_project", {
      path: projectPath,
      registered: true,
      created: false,
    });
    fake.onInvoke("discover_project", async () => {
      discoverCalls += 1;
      if (discoverCalls === 1) return discoveryWithLoop(disabledLoop());
      if (discoverCalls === 2) {
        await staleReload.promise;
        return discoveryWithLoop(disabledLoop());
      }
      if (discoverCalls === 3) {
        await queuedReload.promise;
        throw new Error("queued reload failed");
      }
      return discoveryWithLoop(enabledLoop());
    });
    fake.onInvoke("toggle_loop", () => ({
      summary: enabledLoop(),
      promptBody: "Short preview",
    }));

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
      await waitFor(() => {
        expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
          "enabled",
        );
      });

      staleReload.resolve();
      await waitFor(() => expect(discoverCalls).toBe(3));
      expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
        "enabled",
      );

      queuedReload.resolve();
      await staleReloadPromise;

      expect(rendered.root.querySelector(rowSelector)?.getAttribute("data-ac-state")).toBe(
        "enabled",
      );
    } finally {
      consoleError.mockRestore();
      rendered.cleanup();
    }
  });
});
