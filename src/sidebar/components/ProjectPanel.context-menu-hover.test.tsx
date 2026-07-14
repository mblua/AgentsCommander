// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
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
import { defaultGroupsConfig, workgroupGroupsStore } from "../stores/workgroup-groups";

// #977 - the replica context menu closes when the pointer leaves it, after a
// grace period that any re-entry cancels.
//
// The load-bearing case is the submenu: `Add to Group` (and the repo Browse
// entries, covered in ProjectPanel.repo-browse.test.tsx) render their flyout in
// their OWN <Portal>, so it is NOT a DOM descendant of .session-context-menu.
// Crossing from the menu into a flyout therefore fires the menu's own
// mouseleave, and a close that ignored that would dismiss the menu exactly when
// the user reaches for the submenu. The flyouts cancel the pending close on
// mouseenter, which is what makes "inside a submenu" count as "inside the menu".
//
// GRACE is CONTEXT_MENU_CLOSE_GRACE_MS in ProjectPanel.tsx.
const GRACE = 250;

const projectPath = "C:\\Project";
const wgPath = `${projectPath}\\.ac\\wg-1-dev-team`;
const coordPath = `${wgPath}\\__agent_dev-webpage-ui`;
const memberPath = `${wgPath}\\__agent_dev-rust`;

// A live coordinator renders in the quick section; a non-coordinator member
// renders under workgroups. Both open the full (active) replica menu.
const coordRowTestId = "replica.row.quick.wg-1-dev-team.dev-webpage-ui";
const memberRowTestId = "replica.row.workgroups.wg-1-dev-team.dev-rust";
const groupsTriggerTestId = "replica.wg-1-dev-team.groups.trigger";
const groupsFlyoutTestId = "replica.wg-1-dev-team.groups.flyout";

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wgPath,
        task: null,
        taskTitle: "Hover close",
        agents: [
          {
            name: "dev-webpage-ui",
            path: coordPath,
            identityPath: "../../_agent_dev-webpage-ui",
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: memberPath,
            identityPath: "../../_agent_dev-rust",
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
    ],
  });
}

function liveSessions() {
  return [
    session({
      id: "coord-session",
      name: "wg-1-dev-team/dev-webpage-ui",
      workingDirectory: coordPath,
      status: "running",
      isCoordinator: true,
    }),
    session({
      id: "member-session",
      name: "wg-1-dev-team/dev-rust",
      workingDirectory: memberPath,
      status: "running",
    }),
  ];
}

function menu(): HTMLElement | null {
  // The menu and its flyouts render through a Portal, into document.body.
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function target(testId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
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

describe("ProjectPanel replica context menu - close on pointer leave (#977)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(): Promise<void> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());
    fake.onInvoke("update_project_groups", (args) => args.config);
    sessionsStore.setSessions(liveSessions());
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await workgroupGroupsStore.save(projectPath, defaultGroupsConfig());
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-webpage-ui"));
  }

  async function openCoordinatorMenu(): Promise<HTMLElement> {
    const row = rendered!.root.querySelector(`[data-ac-testid="${coordRowTestId}"]`);
    contextMenu(row!);
    await waitFor(() => expect(menu()).not.toBeNull());
    await flush();
    return menu()!;
  }

  /** Hover the `Add to Group` trigger and wait for its portalled flyout. */
  async function openGroupFlyout(): Promise<HTMLElement> {
    mouse(target(groupsTriggerTestId)!, "mouseenter");
    await waitFor(() => expect(target(groupsFlyoutTestId)).not.toBeNull());
    return target(groupsFlyoutTestId)!;
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

  it("closes the menu once the pointer has been away for the grace period", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      mouse(open, "mouseleave");
      expect(menu()).not.toBeNull(); // still open: the close is only scheduled

      vi.advanceTimersByTime(GRACE - 50);
      expect(menu()).not.toBeNull();

      vi.advanceTimersByTime(100);
      await Promise.resolve();
      expect(menu()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("cancels the close when the pointer comes back within the grace period", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      mouse(open, "mouseleave");
      vi.advanceTimersByTime(GRACE - 50);
      mouse(open, "mouseenter"); // a quick cursor slip, not a dismissal

      vi.advanceTimersByTime(GRACE * 4);
      await Promise.resolve();
      expect(menu()).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the menu open when the pointer moves into the Add to Group flyout", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();
    const flyout = await openGroupFlyout();

    vi.useFakeTimers();
    try {
      // The real pointer path from the trigger into the flyout: the flyout is a
      // sibling Portal, so the MENU sees a mouseleave on the way in.
      mouse(target(groupsTriggerTestId)!, "mouseleave");
      mouse(open, "mouseleave");
      mouse(flyout, "mouseenter");

      vi.advanceTimersByTime(GRACE * 4);
      await Promise.resolve();
      expect(menu()).not.toBeNull();
      expect(target(groupsFlyoutTestId)).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("closes the menu when the pointer leaves the flyout without returning", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();
    const flyout = await openGroupFlyout();

    vi.useFakeTimers();
    try {
      mouse(open, "mouseleave");
      mouse(flyout, "mouseenter");
      vi.advanceTimersByTime(GRACE * 2);
      expect(menu()).not.toBeNull();

      mouse(flyout, "mouseleave");
      vi.advanceTimersByTime(GRACE * 2);
      await Promise.resolve();
      expect(menu()).toBeNull();
      expect(target(groupsFlyoutTestId)).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let a pending close dismiss the NEXT menu", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();

    vi.useFakeTimers();
    try {
      // Slide off the coordinator's menu and right-click another row before the
      // grace period is up. The close was armed by the menu that is already gone.
      mouse(open, "mouseleave");
      vi.advanceTimersByTime(GRACE - 100);

      const memberRow = rendered!.root.querySelector(`[data-ac-testid="${memberRowTestId}"]`);
      contextMenu(memberRow!);
      vi.advanceTimersByTime(GRACE * 4);
      await Promise.resolve();

      const reopened = menu();
      expect(reopened).not.toBeNull();
      // Open Replica's Folder carries the replica path, so the titles identify
      // WHICH replica's menu survived. (A path is not usable in a CSS attribute
      // selector here: its backslashes are escape characters.)
      const titles = Array.from(reopened!.querySelectorAll("button")).map((b) =>
        b.getAttribute("title")
      );
      expect(titles).toContain(memberPath);
      expect(titles).not.toContain(coordPath);
    } finally {
      vi.useRealTimers();
    }
  });

  it("still closes on Escape and on an outside click", async () => {
    await setupPanel();

    await openCoordinatorMenu();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await waitFor(() => expect(menu()).toBeNull());

    await openCoordinatorMenu();
    document.body.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    await waitFor(() => expect(menu()).toBeNull());
  });

  it("renders the restart and coding agent icons", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();

    const labels = Array.from(open.querySelectorAll("button")).map((b) =>
      (b.textContent ?? "").trim()
    );
    expect(labels).toContain("↺ Restart Session");
    expect(labels).toContain("\u{1F916} Coding Agent");
  });
});
