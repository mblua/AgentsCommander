// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../shared/testing/ui-harness";
import { sessionsStore } from "./stores/sessions";
import { liveSelection, SESSION_A, SESSION_B } from "../shared/testing/session-selection";

// #1624 end-to-end: the full SidebarApp keeps the coordinator tile order frozen
// while a context menu is open even after the pointer leaves the sidebar, and
// releases on Escape / outside click.

const projectPath = "C:\\Project";
const wg1Path = `${projectPath}\\.ac\\wg-1-dev-team`;
const coordAPath = `${wg1Path}\\__agent_coord-a`;
const wg2Path = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordBPath = `${wg2Path}\\__agent_coord-b`;

const coordARowTestId = "replica.row.quick.wg-1-dev-team.coord-a";
const coordBRowTestId = "replica.row.quick.wg-2-dev-team.coord-b";

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

function setupTransport(fake: FakeTransport): void {
  fake.resolve(
    "get_settings",
    baseSettings({
      projectPaths: [projectPath],
      projectPath,
    })
  );
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.resolve("get_project_groups", { groups: [], showAll: true, showUngrouped: true });
  fake.resolve("search_repos", []);
  fake.resolve("list_sessions", liveSessions());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
  fake.resolve("list_detached_sessions", []);
  fake.resolve("telegram_list_bridges", []);
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".session-context-menu");
}

/** Document order of the coordinator quick-access rows, testid-to-testid. */
function quickOrder(rendered: { root: HTMLDivElement }): string[] {
  return Array.from(
    rendered.root.querySelectorAll<HTMLElement>('[data-ac-testid^="replica.row.quick."]')
  ).map((el) => el.getAttribute("data-ac-testid")!);
}

/** Drain the macrotask queue. The menu registers its window-level dismiss
 *  listeners inside a setTimeout, so a test that dispatches a dismissing event
 *  without flushing first would hit a menu that is not yet listening. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("SidebarApp sidebar order lock (#1624)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
  });

  it("does not reorder when the pointer moves from a tile onto its open context menu", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelectorAll('[data-ac-testid^="replica.row.quick."]').length).toBe(2);
      });
      sessionsStore.setCoordSortByActivity(true);

      const layout = rendered.root.querySelector(".sidebar-layout")!;
      const coordARow = rendered.root.querySelector(`[data-ac-testid="${coordARowTestId}"]`)!;
      contextMenu(coordARow);
      await waitFor(() => expect(menu()).not.toBeNull());
      await flush(); // real timers: the dismiss listeners are registered

      vi.useFakeTimers();
      try {
        sessionsStore.markActivity(SESSION_A);
        vi.advanceTimersByTime(1000);
        // jsdom has no PointerEvent ctor; Solid binds pointerenter/pointerleave
        // directly, so a plain Event dispatch reaches the handlers.
        layout.dispatchEvent(new Event("pointerenter"));
        layout.dispatchEvent(new Event("pointerleave"));
        await Promise.resolve();
        // The exact #1624 repro state: pointer left the sidebar, menu still open.
        expect(sessionsStore.sidebarPointerInside).toBe(false);
        expect(sessionsStore.sidebarMenuOpen).toBe(true);

        sessionsStore.markActivity(SESSION_B);
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        expect(quickOrder(rendered)).toEqual([coordARowTestId, coordBRowTestId]);

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        await Promise.resolve();
        expect(menu()).toBeNull();
        expect(sessionsStore.sidebarMenuOpen).toBe(false);

        sessionsStore.markActivity(SESSION_B);
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        expect(quickOrder(rendered)).toEqual([coordBRowTestId, coordARowTestId]);
      } finally {
        vi.useRealTimers();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("does not reorder while a menu is open with the pointer outside, and releases on an outside click", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);

    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => {
        expect(rendered.root.querySelectorAll('[data-ac-testid^="replica.row.quick."]').length).toBe(2);
      });
      sessionsStore.setCoordSortByActivity(true);

      const coordARow = rendered.root.querySelector(`[data-ac-testid="${coordARowTestId}"]`)!;
      contextMenu(coordARow);
      await waitFor(() => expect(menu()).not.toBeNull());
      await flush(); // real timers: the dismiss listeners are registered

      vi.useFakeTimers();
      try {
        sessionsStore.markActivity(SESSION_A);
        vi.advanceTimersByTime(1000);
        sessionsStore.markActivity(SESSION_B);
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        // The pointer never entered the sidebar; the menu alone holds the lock.
        expect(quickOrder(rendered)).toEqual([coordARowTestId, coordBRowTestId]);
        expect(sessionsStore.sidebarMenuOpen).toBe(true);

        document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
        await Promise.resolve();
        expect(menu()).toBeNull();
        expect(sessionsStore.sidebarMenuOpen).toBe(false);

        sessionsStore.markActivity(SESSION_B);
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        expect(quickOrder(rendered)).toEqual([coordBRowTestId, coordARowTestId]);
      } finally {
        vi.useRealTimers();
      }
    } finally {
      rendered.cleanup();
    }
  });
});
