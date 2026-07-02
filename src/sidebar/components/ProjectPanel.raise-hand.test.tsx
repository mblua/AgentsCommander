// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { settingsStore } from "../../shared/stores/settings";
import type { AcAgentReplica, AcWorkgroup, Session } from "../../shared/types";

const projectPath = "C:\\Project";
const updatedAt = "2026-06-28T17:00:00.000Z";

function wgPath(wgName: string): string {
  return `${projectPath}\\.ac\\${wgName}`;
}

function replicaPath(wgName: string, replicaName: string): string {
  return `${wgPath(wgName)}\\__agent_${replicaName}`;
}

function rowSlotTestId(wgName: string, replicaName: string): string {
  return `replica.row.quick.${wgName}.${replicaName}.communicationSlot`;
}

function replica(wgName: string, replicaName: string, isCoordinator = true): AcAgentReplica {
  return {
    name: replicaName,
    path: replicaPath(wgName, replicaName),
    repoPaths: [],
    isCoordinator,
  };
}

function workgroup(
  wgName: string,
  replicaName: string,
  taskTitle: string | null,
  isCoordinator = true
): AcWorkgroup {
  return {
    name: wgName,
    path: wgPath(wgName),
    task: null,
    taskTitle,
    agents: [replica(wgName, replicaName, isCoordinator)],
  };
}

function coordSession(
  wgName: string,
  replicaName: string,
  overrides: Partial<Session> = {}
): Session {
  return session({
    id: `${wgName}-${replicaName}`,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath(wgName, replicaName),
    isCoordinator: true,
    status: "running",
    communication: {
      kind: "raiseHand",
      visible: true,
      updatedAt,
    },
    ...overrides,
  });
}

async function mountProject(workgroups: AcWorkgroup[], sessions: Session[]) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", discovery({ workgroups }));
  sessionsStore.setSessions(sessions);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
  return rendered;
}

describe("ProjectPanel raise-hand communication slot (#676)", () => {
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

  it("renders the slot only on the matching coordinator task-title row", async () => {
    const rendered = await mountProject(
      [
        workgroup("wg-1-dev-team", "architect", "First task"),
        workgroup("wg-2-dev-team", "dev-webpage-ui", "Second task"),
      ],
      [
        coordSession("wg-1-dev-team", "architect", { communication: null }),
        coordSession("wg-2-dev-team", "dev-webpage-ui"),
      ]
    );

    try {
      await waitFor(() => {
        expect(
          rendered.root.querySelector(`[data-ac-testid="${rowSlotTestId("wg-2-dev-team", "dev-webpage-ui")}"]`)
        ).not.toBeNull();
      });

      expect(rendered.root.querySelectorAll(".coord-communication-slot")).toHaveLength(1);
      expect(
        rendered.root.querySelector(`[data-ac-testid="${rowSlotTestId("wg-1-dev-team", "architect")}"]`)
      ).toBeNull();

      const slot = rendered.root.querySelector(
        `[data-ac-testid="${rowSlotTestId("wg-2-dev-team", "dev-webpage-ui")}"]`
      );
      const line = slot?.closest(".coord-task-line");
      expect(line?.querySelector(".coord-task-title")?.textContent).toBe("Second task");
    } finally {
      rendered.cleanup();
    }
  });

  it("does not render for null clears, hidden communication, or missing task titles", async () => {
    const wgName = "wg-2-dev-team";
    const replicaName = "dev-webpage-ui";
    const slotSelector = `[data-ac-testid="${rowSlotTestId(wgName, replicaName)}"]`;
    const rendered = await mountProject(
      [workgroup(wgName, replicaName, "Raise task")],
      [coordSession(wgName, replicaName, { communication: null })]
    );

    try {
      expect(rendered.root.querySelector(slotSelector)).toBeNull();

      sessionsStore.setCommunication(`${wgName}-${replicaName}`, {
        kind: "raiseHand",
        visible: false,
        updatedAt,
      });
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).toBeNull());

      sessionsStore.setCommunication(`${wgName}-${replicaName}`, {
        kind: "raiseHand",
        visible: true,
        updatedAt,
      });
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).not.toBeNull());

      sessionsStore.setCommunication(`${wgName}-${replicaName}`, null);
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).toBeNull());

      sessionsStore.setCommunication(`${wgName}-${replicaName}`, {
        kind: "raiseHand",
        visible: true,
        updatedAt,
      });
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).not.toBeNull());

      projectStore.updateWorkgroupTask(wgPath(wgName), null, null);
      await waitFor(() => expect(rendered.root.querySelector(slotSelector)).toBeNull());
    } finally {
      rendered.cleanup();
    }
  });

  it("does not render for non-coordinator rows", async () => {
    const nonCoord = await mountProject(
      [workgroup("wg-3-dev-team", "worker", "Worker task", false)],
      [
        coordSession("wg-3-dev-team", "worker", {
          isCoordinator: false,
        }),
      ]
    );
    try {
      expect(nonCoord.root.querySelector(".coord-communication-slot")).toBeNull();
    } finally {
      nonCoord.cleanup();
    }
  });

  it("renders for a dormant (exited) coordinator session with restored raise-hand communication (#747)", async () => {
    const exited = await mountProject(
      [workgroup("wg-4-dev-team", "dev-webpage-ui", "Exited task")],
      [
        coordSession("wg-4-dev-team", "dev-webpage-ui", {
          status: { exited: 0 },
        }),
      ]
    );
    try {
      await waitFor(() => {
        const slot = exited.root.querySelector(
          `[data-ac-testid="${rowSlotTestId("wg-4-dev-team", "dev-webpage-ui")}"]`
        );
        expect(slot).not.toBeNull();
        expect(slot?.getAttribute("data-kind")).toBe("raiseHand");
      });
    } finally {
      exited.cleanup();
    }
  });
});
