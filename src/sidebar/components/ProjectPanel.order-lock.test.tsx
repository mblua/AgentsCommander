// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { SESSION_A, SESSION_B } from "../../shared/testing/session-selection";
import {
  defaultGroupsConfig,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";

// #1624 - the coordinator tile order stays frozen while a sidebar context menu
// (or its flyout) is open, even with the pointer OUTSIDE the sidebar. The menu
// renders through a Solid <Portal> under document.body, so the store's
// DOM-derived `sidebarMenuOpen` lock (fed by a MutationObserver) is what holds
// the freeze, and every close path releases it structurally.

const projectPath = "C:\\Project";
const wg1Path = `${projectPath}\\.ac\\wg-1-dev-team`;
const coordAPath = `${wg1Path}\\__agent_coord-a`;
const wg2Path = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordBPath = `${wg2Path}\\__agent_coord-b`;

const coordARowTestId = "replica.row.quick.wg-1-dev-team.coord-a";
const coordBRowTestId = "replica.row.quick.wg-2-dev-team.coord-b";
const groupsTriggerTestId = "replica.wg-1-dev-team.groups.trigger";
const groupsFlyoutTestId = "replica.wg-1-dev-team.groups.flyout";

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wg1Path,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "coord-a",
            path: coordAPath,
            identityPath: "../../_agent_coord-a",
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
      {
        name: "wg-2-dev-team",
        path: wg2Path,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "coord-b",
            path: coordBPath,
            identityPath: "../../_agent_coord-b",
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function liveSessions() {
  return [
    session({
      id: SESSION_A,
      name: "wg-1-dev-team/coord-a",
      workingDirectory: coordAPath,
      status: "running",
      isCoordinator: true,
    }),
    session({
      id: SESSION_B,
      name: "wg-2-dev-team/coord-b",
      workingDirectory: coordBPath,
      status: "running",
      isCoordinator: true,
    }),
  ];
}

/** The menus and their flyouts render through a Portal, into document.body. */
function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function flyout(): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${groupsFlyoutTestId}"]`);
}

// Solid does not delegate mouseenter/mouseleave (they do not bubble); it binds
// them directly, so a non-bubbling dispatch is exactly what the handler sees.
function mouse(el: Element, type: "mouseenter" | "mouseleave"): void {
  el.dispatchEvent(new MouseEvent(type, { bubbles: false, cancelable: true }));
}

/** Drain the macrotask queue. The menu registers its window-level dismiss
 *  listeners inside a setTimeout, so a test that dispatches a dismissing event
 *  without flushing first would hit a menu that is not yet listening. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("ProjectPanel sidebar order lock (#1624)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  /** Document order of the coordinator quick-access rows, testid-to-testid. */
  function quickOrder(): string[] {
    return Array.from(
      rendered!.root.querySelectorAll<HTMLElement>('[data-ac-testid^="replica.row.quick."]')
    ).map((el) => el.getAttribute("data-ac-testid")!);
  }

  async function setupPanel(): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());
    fake.onInvoke("update_project_groups", (args) => args.config);
    // The "Coding Agent" action mounts AgentPickerModal, whose onMount begins
    // with an unguarded SettingsAPI.get() (AgentPickerModal.tsx:288-290).
    fake.resolve("get_settings", baseSettings());
    // Test "releases when the project is removed" awaits ProjectAPI.remove,
    // which is an unguarded transport.invoke("remove_project").
    fake.resolve("remove_project", undefined);
    sessionsStore.setSessions(liveSessions());
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await workgroupGroupsStore.save(projectPath, defaultGroupsConfig());
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("coord-a"));
    sessionsStore.setCoordSortByActivity(true);
    sessionsStore.setSidebarPointerInside(false);
    return fake;
  }

  /** Open the coord-a replica menu under REAL timers and drain the setTimeout
   *  that registers the window dismiss listeners and positions the menu. */
  async function openCoordinatorMenu(): Promise<void> {
    const row = rendered!.root.querySelector(`[data-ac-testid="${coordARowTestId}"]`);
    contextMenu(row!);
    await waitFor(() => expect(menu()).not.toBeNull());
    await flush();
  }

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
    document.body.replaceChildren();
  });

  it("keeps the coordinator order frozen while a context menu is open even with the pointer outside", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      sessionsStore.setSidebarPointerInside(false);
      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordARowTestId, coordBRowTestId]);
      expect(sessionsStore.sidebarMenuOpen).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("releases on Escape", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordARowTestId, coordBRowTestId]);

      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordBRowTestId, coordARowTestId]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("releases on an outside click", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordARowTestId, coordBRowTestId]);

      document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordBRowTestId, coordARowTestId]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("releases when a menu action closes the menu and opens a modal (modals are out of scope)", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordARowTestId, coordBRowTestId]);

      const codingAgentButton = Array.from(
        menu()!.querySelectorAll<HTMLButtonElement>(".session-context-option")
      ).find((button) => (button.textContent ?? "").includes("Coding Agent"));
      expect(codingAgentButton).toBeDefined();
      click(codingAgentButton!);
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(document.querySelector(".modal-overlay")).not.toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordBRowTestId, coordARowTestId]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the lock across menu replacement", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      // The replica menu node is removed and the project menu node inserted in
      // the same task; the observer recomputes presence and stays locked.
      contextMenu(rendered!.root.querySelector(".project-header")!);
      await Promise.resolve();
      const replaced = menu();
      expect(replaced).not.toBeNull();
      const labels = Array.from(replaced!.querySelectorAll("button")).map((b) =>
        (b.textContent ?? "").trim()
      );
      expect(labels).toContain("New Agent");
      expect(labels.some((label) => label.includes("Restart Session"))).toBe(false);
      expect(sessionsStore.sidebarMenuOpen).toBe(true);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordARowTestId, coordBRowTestId]);
    } finally {
      vi.useRealTimers();
    }

    // Release tail. The replacement menu's dismiss listeners were queued on the
    // fake clock and are lost on useRealTimers, so re-open it under real timers
    // to re-register, then close via Escape.
    contextMenu(rendered!.root.querySelector(".project-header")!);
    await flush();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await waitFor(() => expect(menu()).toBeNull());
    await Promise.resolve();
    expect(sessionsStore.sidebarMenuOpen).toBe(false);
  });

  it("releases when the project is removed while a menu is open", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      await projectStore.removeProject(projectPath);
      await Promise.resolve();
      expect(rendered!.root.querySelector(".project-panel")).toBeNull();
      expect(menu()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("releases through the replica-menu grace timer (250 ms) with the pointer outside", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);
      sessionsStore.setSidebarPointerInside(false); // the menu alone holds the lock
      mouse(menu()!, "mouseleave"); // arms scheduleReplicaCtxMenuClose (250 ms)
      expect(menu()).not.toBeNull(); // the close is only scheduled

      vi.advanceTimersByTime(300);
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordBRowTestId, coordARowTestId]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("releases the group flyout on its 180 ms timer while the parent menu lock holds, then releases the parent on its own timer", async () => {
    await setupPanel();
    await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      sessionsStore.markActivity(SESSION_A);
      vi.advanceTimersByTime(1000);

      const trigger = document.querySelector<HTMLElement>(
        `[data-ac-testid="${groupsTriggerTestId}"]`
      );
      mouse(trigger!, "mouseenter"); // mounts the Add to Group flyout
      await Promise.resolve();
      const openFlyout = flyout();
      expect(openFlyout).not.toBeNull();

      mouse(openFlyout!, "mouseenter"); // cancels (nothing armed yet)
      mouse(openFlyout!, "mouseleave"); // arms 180 ms flyout close + 250 ms menu close
      vi.advanceTimersByTime(200); // past 180, before 250
      await Promise.resolve();
      expect(flyout()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(true); // parent menu still in the DOM

      mouse(menu()!, "mouseleave"); // re-arms the 250 ms parent close
      vi.advanceTimersByTime(300);
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(sessionsStore.sidebarMenuOpen).toBe(false);

      sessionsStore.markActivity(SESSION_B);
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
      expect(quickOrder()).toEqual([coordBRowTestId, coordARowTestId]);
    } finally {
      vi.useRealTimers();
    }
  });
});
