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
const updatedAt = "2026-08-31T06:00:00.000Z";
const menuMessage = "Choose the authentication method";

function wgPath(wgName: string): string {
  return `${projectPath}\\.ac\\${wgName}`;
}

function replicaPath(wgName: string, replicaName: string): string {
  return `${wgPath(wgName)}\\__agent_${replicaName}`;
}

function rowSlotTestId(
  wgName: string,
  replicaName: string,
  rowContext = "quick"
): string {
  return `replica.row.${rowContext}.${wgName}.${replicaName}.communicationSlot`;
}

function replica(wgName: string, replicaName: string, isCoordinator: boolean): AcAgentReplica {
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
  isCoordinator: boolean
): AcWorkgroup {
  return {
    name: wgName,
    path: wgPath(wgName),
    task: null,
    taskTitle,
    agents: [replica(wgName, replicaName, isCoordinator)],
  };
}

function blockedMenuSession(
  wgName: string,
  replicaName: string,
  isCoordinator: boolean,
  message = menuMessage
): Session {
  return session({
    id: `${wgName}-${replicaName}`,
    name: `${wgName}/${replicaName}`,
    workingDirectory: replicaPath(wgName, replicaName),
    isCoordinator,
    status: "running",
    communication: {
      kind: "blockedMenu",
      visible: true,
      updatedAt,
      message,
    },
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

describe("ProjectPanel blocked-menu communication slot (#1649)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: Awaited<ReturnType<typeof mountProject>> | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    rendered?.cleanup();
    rendered = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("renders the blocked-menu slot for a coordinator replica", async () => {
    const wgName = "room-coord";
    const replicaName = "orchestrator";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, "Coordinate", true)],
      [blockedMenuSession(wgName, replicaName, true)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName)}"]`
    );
    expect(slot?.getAttribute("data-kind")).toBe("blockedMenu");
    const strip = slot?.parentElement;
    expect(strip?.classList.contains("ac-discovery-badges")).toBe(true);
    expect(strip?.firstElementChild).toBe(slot);
    expect(rendered.root.querySelector(".replica-item-name-row")).toBeNull();
    expect(rendered.root.querySelector(".coord-task-line")).not.toBeNull();
  });

  it("renders the blocked-menu slot for a worker without a task title", async () => {
    const wgName = "room-worker";
    const replicaName = "dev-rust";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, null, false)],
      [blockedMenuSession(wgName, replicaName, false)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName, "workgroups")}"]`
    );
    expect(slot?.getAttribute("data-kind")).toBe("blockedMenu");
    const strip = slot?.parentElement;
    expect(strip?.classList.contains("ac-discovery-badges")).toBe(true);
    expect(strip?.firstElementChild).toBe(slot);
    const chip = strip?.querySelector<HTMLElement>(".agent-name-chip");
    expect(chip?.textContent).toBe(replicaName);
    expect(chip?.getAttribute("title")).toBe(replicaName);
    expect(rendered.root.querySelector(".replica-item-name-row")).toBeNull();
  });

  it("uses the backend message for the tooltip and exposes an accessible label", async () => {
    const wgName = "room-message";
    const replicaName = "architect";
    rendered = await mountProject(
      [workgroup(wgName, replicaName, null, false)],
      [blockedMenuSession(wgName, replicaName, false)]
    );

    const slot = rendered.root.querySelector(
      `[data-ac-testid="${rowSlotTestId(wgName, replicaName, "workgroups")}"]`
    );
    expect(slot?.getAttribute("title")).toBe(menuMessage);
    expect(slot?.getAttribute("aria-label")).toBe("Interactive menu requires user input");
  });
});
