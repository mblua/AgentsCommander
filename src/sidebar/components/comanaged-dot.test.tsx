// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SessionItem from "./SessionItem";
import RootAgentBanner from "./RootAgentBanner";
import ProjectPanel from "./ProjectPanel";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import { projectStore, type ProjectState } from "../stores/project";
import { defaultGroupsConfig } from "../stores/workgroup-groups";
import { sessionsStore } from "../stores/sessions";
import type { Session } from "../../shared/types";

// #2271 phase 8 - the three components that paint the circle today must carry
// the new state: SessionItem, RootAgentBanner and the ProjectPanel replica row.
// The Co-managed flag lives in the store's sidecar map, keyed by session id, so
// every leg drives the map through the public setter instead of a Session field.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const replicaAgentPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const REPLICA_SESSION_ID = "comanaged-replica-session";

const replicaRowSelector = `[data-ac-testid="replica.row.workgroups.wg-2-dev-team.dev-webpage-ui"]`;

function projectDiscovery() {
  return discovery({
    agents: [
      { name: "static-agent", path: `${projectPath}\\.ac\\_agent_static`, roleExists: true },
    ],
    teams: [],
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "dev-webpage-ui",
            path: replicaAgentPath,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function replicaSession(overrides: Partial<Session> = {}): Session {
  return session({
    id: REPLICA_SESSION_ID,
    name: "wg-2-dev-team/dev-webpage-ui",
    workingDirectory: replicaAgentPath,
    status: "running",
    // Today's search text for this row would be `waiting`; test 17 proves the
    // comanaged flag replaces it and that clearing it restores `waiting`.
    waitingForInput: true,
    ...overrides,
  });
}

function findByTestId<T extends Element>(root: ParentNode, testId: string): T {
  const el = root.querySelector(`[data-ac-testid="${testId}"]`);
  if (!el) throw new Error(`Element not found: ${testId}`);
  return el as T;
}

function replicaRow(root: ParentNode): HTMLElement {
  const row = root.querySelector<HTMLElement>(replicaRowSelector);
  if (!row) throw new Error("replica row not rendered");
  return row;
}

function dotOf(row: Element): HTMLElement {
  const dot = row.querySelector<HTMLElement>(".session-item-status");
  if (!dot) throw new Error("status dot not rendered");
  return dot;
}

function itemRow(root: ParentNode, id: string): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="session.${id}"]`);
  if (!el) throw new Error(`session row ${id} not rendered`);
  return el;
}

async function renderPanel() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => {
    replicaRow(rendered.root);
  });
  return rendered;
}

function railProject(): ProjectState {
  return {
    path: projectPath,
    folderName: "Project",
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: `${projectPath}\\.ac\\wg-1-dev-team`,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

describe("Co-managed dot across the three render sites (#2271)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
  });

  afterEach(() => {
    sessionsStore.resetComanagedForTests();
    resetUiStoresForTests();
    cleanupDom?.();
    cleanupDom = null;
    document.body.replaceChildren();
  });

  it("marks the SessionItem dot and preserves the container data-ac-state (test 12)", async () => {
    const s = session({ id: "cm-item", name: "wg-1-dev-team/architect", status: "running" });
    sessionsStore.setSessions([s]);
    sessionsStore.setSessionComanaged(s.id, true);

    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    const rendered = renderWithFakeTransport(
      () => <SessionItem session={s} isActive={false} />,
      fake,
    );
    try {
      await settingsStore.load();
      const row = itemRow(rendered.root, s.id);
      const dot = dotOf(row);
      const stateBefore = row.getAttribute("data-ac-state");
      expect(stateBefore).toBe("idle");

      expect(dot.classList.contains("comanaged")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("true");
      expect(dot.getAttribute("title")).toMatch(/co-managed/i);

      sessionsStore.setSessionComanaged(s.id, false);
      await waitFor(() => expect(dot.classList.contains("comanaged")).toBe(false));
      expect(dot.classList.contains("running")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");
      expect(dot.hasAttribute("title")).toBe(false);
      // No overload: the selection vocabulary on the container is untouched.
      expect(row.getAttribute("data-ac-state")).toBe(stateBefore);
    } finally {
      rendered.cleanup();
    }
  });

  it("marks the RootAgentBanner dot and preserves the container data-ac-state (test 12)", async () => {
    sessionsStore.setSessions([
      session({ id: "root-1", name: "Agent's Commander", isRootAgent: true, status: "running" }),
    ]);
    sessionsStore.setSessionComanaged("root-1", true);

    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, new FakeTransport());
    try {
      const row = rendered.root.querySelector<HTMLElement>(".root-agent-banner");
      if (!row) throw new Error("root agent banner not rendered");
      const dot = dotOf(row);
      const stateBefore = row.getAttribute("data-ac-state");
      // PTY liveness vocabulary, NOT status.
      expect(stateBefore).toBe("live");

      expect(dot.classList.contains("comanaged")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("true");
      expect(dot.getAttribute("title")).toMatch(/co-managed/i);

      sessionsStore.setSessionComanaged("root-1", false);
      await waitFor(() => expect(dot.classList.contains("comanaged")).toBe(false));
      expect(dot.classList.contains("running")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");
      expect(row.getAttribute("data-ac-state")).toBe(stateBefore);
    } finally {
      rendered.cleanup();
    }
  });

  it("marks the ProjectPanel replica dot, whose row carries no data-ac-state at all (test 12)", async () => {
    sessionsStore.setSessions([replicaSession()]);
    sessionsStore.setSessionComanaged(REPLICA_SESSION_ID, true);

    const rendered = await renderPanel();
    try {
      const row = replicaRow(rendered.root);
      const dot = dotOf(row);
      expect(row.hasAttribute("data-ac-state")).toBe(false);

      expect(dot.classList.contains("comanaged")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("true");
      expect(dot.getAttribute("title")).toMatch(/co-managed/i);

      sessionsStore.setSessionComanaged(REPLICA_SESSION_ID, false);
      await waitFor(() => expect(dot.classList.contains("comanaged")).toBe(false));
      expect(dot.classList.contains("waiting")).toBe(true);
      expect(dot.getAttribute("data-ac-comanaged")).toBe("false");
      expect(dot.hasAttribute("title")).toBe(false);
      // Still no invented attribute on the row after the state changed.
      expect(row.hasAttribute("data-ac-state")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("clearing the flag returns the row to the state it would otherwise have had, including waiting (test 13)", async () => {
    const waiting = session({
      id: "cm-waiting",
      name: "wg-1-dev-team/waiting",
      status: "running",
      waitingForInput: true,
    });
    const pending = session({
      id: "cm-pending",
      name: "wg-1-dev-team/pending",
      status: "running",
      pendingReview: true,
    });
    sessionsStore.setSessions([waiting, pending]);
    sessionsStore.setSessionComanaged(waiting.id, true);
    sessionsStore.setSessionComanaged(pending.id, true);

    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    const rendered = renderWithFakeTransport(
      () => (
        <>
          <SessionItem session={waiting} isActive={false} />
          <SessionItem session={pending} isActive={false} />
        </>
      ),
      fake,
    );
    try {
      await settingsStore.load();
      const waitingDot = dotOf(itemRow(rendered.root, waiting.id));
      const pendingDot = dotOf(itemRow(rendered.root, pending.id));
      expect(waitingDot.classList.contains("comanaged")).toBe(true);
      expect(pendingDot.classList.contains("comanaged")).toBe(true);

      sessionsStore.setSessionComanaged(waiting.id, false);
      sessionsStore.setSessionComanaged(pending.id, false);

      await waitFor(() => expect(waitingDot.classList.contains("waiting")).toBe(true));
      expect(waitingDot.classList.contains("comanaged")).toBe(false);
      expect(pendingDot.classList.contains("pending")).toBe(true);
      expect(pendingDot.classList.contains("comanaged")).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("the rail button and the static agent rows never receive comanaged (test 9)", async () => {
    // Rail leg: a working room whose session carries the flag. The rail's dot is
    // a static `running` circle, hardcoded, and must stay that way.
    sessionsStore.setSessions([
      session({
        id: "rail-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${projectPath}\\.ac\\wg-1-dev-team\\__agent_dev-webpage-ui`,
        status: "running",
      }),
    ]);
    sessionsStore.setSessionComanaged("rail-session", true);

    const railFake = new FakeTransport();
    railFake.resolve("get_project_groups", defaultGroupsConfig());
    const rail = renderWithFakeTransport(
      () => <WorkgroupGroupRail projects={[railProject()]} />,
      railFake,
    );
    try {
      await waitFor(() =>
        expect(rail.root.querySelector('[data-ac-testid="workgroupGroups.dot.all"]')).not.toBeNull(),
      );
      const railDot = findByTestId<HTMLElement>(rail.root, "workgroupGroups.dot.all");
      expect(railDot.classList.contains("running")).toBe(true);
      expect(railDot.classList.contains("comanaged")).toBe(false);
      expect(railDot.hasAttribute("data-ac-comanaged")).toBe(false);
    } finally {
      rail.cleanup();
    }

    // Static agent row leg: the Agents section fallback has no live session, so
    // its dot is the hardcoded `offline` circle; the live replica row in the
    // same render does show comanaged, so the difference is the row, not a
    // missing flag.
    sessionsStore.setSessions([replicaSession()]);
    sessionsStore.setSessionComanaged(REPLICA_SESSION_ID, true);
    const panel = await renderPanel();
    try {
      expect(dotOf(replicaRow(panel.root)).classList.contains("comanaged")).toBe(true);

      await waitFor(() => expect(panel.root.textContent).toContain("static-agent"));
      const staticRow = Array.from(panel.root.querySelectorAll<HTMLElement>(".replica-item")).find(
        (el) => el.textContent?.includes("static-agent"),
      );
      if (!staticRow) throw new Error("static agent row not rendered");
      const staticDot = dotOf(staticRow);
      expect(staticDot.classList.contains("offline")).toBe(true);
      expect(staticDot.classList.contains("comanaged")).toBe(false);
      expect(staticDot.hasAttribute("data-ac-comanaged")).toBe(false);
    } finally {
      panel.cleanup();
    }
  });

  it("the replica row is searchable as `comanaged`, and not once the flag clears (test 17)", async () => {
    sessionsStore.setSessions([replicaSession()]);
    sessionsStore.setSessionComanaged(REPLICA_SESSION_ID, true);

    const rendered = await renderPanel();
    try {
      const toggle = findByTestId<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = findByTestId<HTMLInputElement>(rendered.root, "project.regexFilter.input");
      await waitFor(() => expect(toggle.getAttribute("aria-expanded")).toBe("true"));

      // With the sidecar flag threaded into sessionEffectiveStatusSearchText,
      // the row's search text is `comanaged` and the filter keeps it.
      input(filterInput, "comanaged");
      await waitFor(() => {
        replicaRow(rendered.root);
      });
      expect(rendered.root.textContent).toContain("dev-webpage-ui");

      // Clear the filter, then the flag: the search text goes back to what it is
      // today (`waiting`), which is what the dot shows again.
      input(filterInput, "");
      sessionsStore.setSessionComanaged(REPLICA_SESSION_ID, false);
      await waitFor(() => expect(dotOf(replicaRow(rendered.root)).classList.contains("waiting")).toBe(true));

      input(filterInput, "comanaged");
      await waitFor(() => {
        expect(rendered.root.querySelector(replicaRowSelector)).toBeNull();
      });
      expect(rendered.root.textContent).not.toContain("dev-webpage-ui");
    } finally {
      rendered.cleanup();
    }
  });
});
