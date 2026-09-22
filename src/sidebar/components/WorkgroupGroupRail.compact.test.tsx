// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "../App";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  registerCompactHostForTests,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { liveSelection, SESSION_A } from "../../shared/testing/session-selection";
import {
  defaultGroupsConfig,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
  type WorkgroupGroupSelection,
} from "../stores/workgroup-groups";
import { projectCollapseStore } from "../stores/project-collapse";
import { setSidebarCompactMode, sidebarCompact } from "../../shared/sidebar-compact";

// #2236 phase 6 — rail autoexpansion. The rail needs the real tree around it,
// so every test mounts SidebarApp embedded and drives compact mode through the
// phase-2 harness (D11: a mode change is a no-op without a registered host).
// Width restoration is deliberately not asserted here: sidebarWidth() is a
// MainApp closure this tree never renders; phase 2's App.compact.test.tsx owns it.

const projectPath = "C:\\Project";

function projectDiscovery() {
  return discovery({
    workgroups: ["wg-1-dev-team", "wg-2-rust-team"].map((name) => ({
      name,
      path: `${projectPath}\\.ac\\${name}`,
      task: null,
      taskTitle: null,
      agents: [],
    })),
  });
}

function groupsConfig() {
  return {
    ...defaultGroupsConfig(),
    groups: [
      { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
      { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
    ],
  };
}

/** The reads SidebarApp needs before its rail can render a project. */
function setupAppReads(fake: FakeTransport): void {
  fake.resolve("get_settings", baseSettings({ projectPaths: [projectPath], projectPath }));
  fake.resolve("open_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.resolve("get_project_groups", groupsConfig());
  fake.resolve("get_active_session", liveSelection(SESSION_A));
}

/** Quiet the panels that mount around the rail; nothing here asserts them. */
function quietSurroundingPanels(fake: FakeTransport): void {
  fake.resolve("list_sessions", []);
  fake.resolve("list_detached_sessions", []);
  fake.resolve("search_repos", []);
  fake.resolve("telegram_list_bridges", []);
  fake.resolve("get_update_status", null);
  fake.resolve("drain_session_warnings", []);
  fake.resolve("get_resource_snapshot", null);
}

function setupTransport(fake: FakeTransport): void {
  setupAppReads(fake);
  quietSurroundingPanels(fake);
}

function uiRailButton(): HTMLElement {
  const element = document.querySelector<HTMLElement>(
    '[data-ac-testid="workgroupGroups.button.ui"]',
  );
  if (!element) throw new Error("missing workgroupGroups.button.ui");
  return element;
}

const uiSelection = (): WorkgroupGroupSelection => ({ kind: "group", id: "ui" });

async function mountRail() {
  const fake = new FakeTransport();
  setupTransport(fake);
  const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
  await waitFor(() => expect(uiRailButton()).not.toBeNull());
  return rendered;
}

describe("WorkgroupGroupRail compact autoexpansion (#2236)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  it("expands when a not-yet-selected group is clicked while compact", async () => {
    registerCompactHostForTests();
    const rendered = await mountRail();
    try {
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);

      click(uiRailButton());

      expect(sidebarCompact()).toBe(false);
      expect(workgroupGroupsStore.selection(projectPath)).toEqual(uiSelection());
    } finally {
      rendered.cleanup();
    }
  });

  it("expands when the already-selected group is clicked while compact", async () => {
    registerCompactHostForTests();
    const rendered = await mountRail();
    try {
      click(uiRailButton());
      await waitFor(() => expect(workgroupGroupsStore.selection(projectPath)).toEqual(uiSelection()));

      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);

      click(uiRailButton());

      expect(sidebarCompact()).toBe(false);
      expect(workgroupGroupsStore.selection(projectPath)).toEqual(uiSelection());
    } finally {
      rendered.cleanup();
    }
  });

  it("routes the click through setSidebarCompactMode before the flip", async () => {
    const observed: Array<{ next: boolean; compact: boolean }> = [];
    registerCompactHostForTests({
      onBeforeModeChange: (next) =>
        observed.push({ next, compact: sidebarCompact() }),
    });
    const rendered = await mountRail();
    try {
      setSidebarCompactMode(true);
      observed.length = 0;

      click(uiRailButton());

      // The pre-flip order only setSidebarCompactMode produces: the hook sees
      // `next === false` while the signal still reads true.
      expect(observed).toEqual([{ next: false, compact: true }]);
      expect(sidebarCompact()).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the selection and collapse calls identical to the expanded route", async () => {
    const collapseSpy = vi.spyOn(projectCollapseStore, "collapseAllExceptKnown");
    const setCollapsedSpy = vi.spyOn(projectCollapseStore, "setProjectCollapsed");
    registerCompactHostForTests();
    const rendered = await mountRail();
    try {
      collapseSpy.mockClear();
      setCollapsedSpy.mockClear();

      click(uiRailButton());
      expect(collapseSpy).toHaveBeenCalledTimes(1);
      expect(setCollapsedSpy).toHaveBeenCalledTimes(1);
      const expandedCollapseCall = collapseSpy.mock.calls[0];
      const expandedSetCall = setCollapsedSpy.mock.calls[0];

      collapseSpy.mockClear();
      setCollapsedSpy.mockClear();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);

      click(uiRailButton());

      // Same two calls, same arguments as the expanded route; autoexpansion
      // adds behaviour and changes none.
      expect(collapseSpy.mock.calls).toEqual([expandedCollapseCall]);
      expect(setCollapsedSpy.mock.calls).toEqual([expandedSetCall]);
      expect(collapseSpy).toHaveBeenCalledWith(projectPath, [projectPath]);
      expect(setCollapsedSpy).toHaveBeenCalledWith(projectPath, false);
      expect(workgroupGroupsStore.selection(projectPath)).toEqual(uiSelection());
    } finally {
      rendered.cleanup();
    }
  });

  it("fires no mode change when a group is clicked while expanded", async () => {
    const onBeforeModeChange = vi.fn();
    registerCompactHostForTests({ onBeforeModeChange });
    const rendered = await mountRail();
    try {
      expect(sidebarCompact()).toBe(false);
      onBeforeModeChange.mockClear();

      click(uiRailButton());

      expect(onBeforeModeChange).not.toHaveBeenCalled();
      expect(sidebarCompact()).toBe(false);
      expect(workgroupGroupsStore.selection(projectPath)).toEqual(uiSelection());
    } finally {
      rendered.cleanup();
    }
  });
});
