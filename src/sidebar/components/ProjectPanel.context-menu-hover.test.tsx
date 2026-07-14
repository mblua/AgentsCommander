// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
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
import {
  defaultGroupsConfig,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";
import type { WorkgroupGroupsConfig } from "../../shared/types";

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
const grayPath = `${wgPath}\\__agent_dev-docs`;

// A live coordinator renders in the quick section; the other replicas render
// under workgroups. dev-docs has no session at all, so it opens the INACTIVE
// (gray) branch of the menu; the other two open the active branch.
const coordRowTestId = "replica.row.quick.wg-1-dev-team.dev-webpage-ui";
const memberRowTestId = "replica.row.workgroups.wg-1-dev-team.dev-rust";
const grayRowTestId = "replica.row.workgroups.wg-1-dev-team.dev-docs";
const groupsTriggerTestId = "replica.wg-1-dev-team.groups.trigger";
const groupsFlyoutTestId = "replica.wg-1-dev-team.groups.flyout";
const groupsErrorTestId = "replica.groups.error";
const groupOptionTestId = "replica.wg-1-dev-team.groups.frontend";

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
          {
            name: "dev-docs",
            path: grayPath,
            identityPath: "../../_agent_dev-docs",
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

/** A group the coordinator's workgroup matches by an EXACT token, so its menu
 *  option is enabled and clicking it is a real (removing) groups write. */
function removableGroupsConfig(): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    groups: [
      {
        id: "frontend",
        name: "Frontend",
        regex: exactGroupRegexForWorkgroup("wg-1-dev-team"),
      },
    ],
  };
}

function menu(): HTMLElement | null {
  // The menu and its flyouts render through a Portal, into document.body.
  return document.querySelector<HTMLElement>(".session-context-menu");
}

function target(testId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

function menuLabels(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll("button")).map((b) => (b.textContent ?? "").trim());
}

// Solid does not delegate mouseenter/mouseleave (they do not bubble); it binds
// them directly, so a non-bubbling dispatch is exactly what the handler sees.
function mouse(el: Element, type: "mouseenter" | "mouseleave"): void {
  el.dispatchEvent(new MouseEvent(type, { bubbles: false, cancelable: true }));
}

function domRect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

/** Drain the macrotask queue. The menu registers its window-level dismiss
 *  listeners inside a setTimeout, so a test that dispatches a dismissing event
 *  without flushing first would hit a menu that is not yet listening. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("ProjectPanel replica context menu - close on pointer leave (#977)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport> | null = null;

  async function setupPanel(groups = defaultGroupsConfig()): Promise<FakeTransport> {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());
    fake.onInvoke("update_project_groups", (args) => args.config);
    sessionsStore.setSessions(liveSessions());
    rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await workgroupGroupsStore.save(projectPath, groups);
    await projectStore.createAndLoad(projectPath);
    await waitFor(() => expect(rendered!.root.textContent).toContain("dev-webpage-ui"));
    return fake;
  }

  async function openMenuOn(rowTestId: string): Promise<HTMLElement> {
    const row = rendered!.root.querySelector(`[data-ac-testid="${rowTestId}"]`);
    contextMenu(row!);
    await waitFor(() => expect(menu()).not.toBeNull());
    await flush();
    return menu()!;
  }

  const openCoordinatorMenu = () => openMenuOn(coordRowTestId);

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

  // The close bails while the Add to Group flyout is showing an error, so the
  // message stays readable (closing the menu would dispose the flyout with it).
  // That bail is scoped to "the error is ON SCREEN", which means the flyout is
  // open - NOT to the bare workgroupGroupsStore.error flag. The next test is why.
  it("keeps the menu open while the Add to Group flyout is showing an error", async () => {
    const fake = await setupPanel(removableGroupsConfig());
    fake.reject("update_project_groups", "groups.toml changed on disk");

    const open = await openCoordinatorMenu();
    const flyout = await openGroupFlyout();
    click(target(groupOptionTestId)!); // removes the workgroup: the write fails
    await waitFor(() => {
      expect(target(groupsErrorTestId)?.textContent).toContain("groups.toml changed on disk");
    });

    vi.useFakeTimers();
    try {
      mouse(open, "mouseleave"); // the pointer is in the flyout, which is a Portal
      mouse(flyout, "mouseleave");
      vi.advanceTimersByTime(GRACE * 4); // PAST the grace, not just up to it
      await Promise.resolve();

      expect(menu()).not.toBeNull();
      expect(target(groupsFlyoutTestId)).not.toBeNull();
      expect(target(groupsErrorTestId)).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  // workgroupGroupsStore.error is a sticky PROJECT-WIDE flag: any failed groups
  // write (another workgroup, the Edit-groups modal) or an invalid groups.toml at
  // load sets it, and only a LATER successful save clears it. If the close bailed
  // on that flag bare, one failed write anywhere would silently disable the
  // pointer-leave close for every replica menu in the project - #977 all over again.
  it("still closes after a failed groups write when the flyout was never opened", async () => {
    const fake = await setupPanel();
    fake.reject("update_project_groups", "groups.toml changed on disk");
    await workgroupGroupsStore.save(projectPath, defaultGroupsConfig()).catch(() => {});
    expect(workgroupGroupsStore.error(projectPath)).toBeTruthy();

    const open = await openCoordinatorMenu(); // the flyout is never opened

    vi.useFakeTimers();
    try {
      mouse(open, "mouseleave");
      vi.advanceTimersByTime(GRACE * 4);
      await Promise.resolve();
      expect(menu()).toBeNull();
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

  // The close depends on the pointer's hover chain reaching the LIVE menu node:
  // mouseleave is only delivered to the element the pointer is actually inside.
  // positionReplicaCtxMenu writes a NEW menu object ({...current, x, y}) ~0ms after
  // every open and again on every reclamp, so if `{replicaCtxMenu() && <Portal>}`
  // re-created the menu on that write, the cursor would be left hovering a detached
  // node and no mouseleave would ever fire in a real browser. It does not: Solid's
  // wrapConditionals memoizes the `&&` on the condition's TRUTHINESS, so an identity
  // change to the object only re-runs the fine-grained style effect. Lock that in.
  it("does not re-create the menu element when the reposition write lands", async () => {
    // A tall menu against jsdom's 768px viewport: the y=96 open position is clamped
    // up to 768 - 700 - 8 = 60, so the reposition write visibly MOVES the menu. That
    // is the control - it proves the new object really did reach the DOM.
    const rects = vi
      .spyOn(Element.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: Element) {
        if (this.classList?.contains("session-context-menu")) return domRect(0, 0, 400, 700);
        return domRect(0, 0, 0, 0);
      });
    try {
      await setupPanel();

      const row = rendered!.root.querySelector(`[data-ac-testid="${coordRowTestId}"]`);
      contextMenu(row!); // the harness dispatches clientX 80 / clientY 96
      await waitFor(() => expect(menu()).not.toBeNull());

      const first = menu()!;
      expect(first.getAttribute("style")).toContain("top: 96px");

      await flush(); // runs the queued positionReplicaCtxMenu

      expect(menu()!.getAttribute("style")).toContain("top: 60px"); // the write landed
      expect(menu()).toBe(first); // ...and the element survived it
      expect(first.isConnected).toBe(true);
    } finally {
      rects.mockRestore();
    }
  });

  it("renders the restart and coding agent icons", async () => {
    await setupPanel();
    const open = await openCoordinatorMenu();

    const labels = menuLabels(open);
    expect(labels).toContain("↺ Restart Session");
    expect(labels).toContain("\u{1F916} Coding Agent");
  });

  it("renders the coding agent icon on the inactive (gray) branch too", async () => {
    await setupPanel();
    const open = await openMenuOn(grayRowTestId);

    const labels = menuLabels(open);
    expect(labels).toContain("\u{1F916} Coding Agent");
    // The gray branch has no Restart Session at all, so this really is that menu.
    expect(labels.some((label) => label.includes("Restart Session"))).toBe(false);
  });
});
