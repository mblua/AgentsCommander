// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Component } from "solid-js";
import type { AcDiscoveryResult, WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";
import { projectCollapseStore } from "../stores/project-collapse";
import {
  activeWorkgroupGroupSelectionKey,
  createSidebarSelectionScrollReset,
} from "../App";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import ProjectPanel from "./ProjectPanel";

// #810 - Two backslash Windows paths (grinch F4). Forward-slash or simplified
// paths would hide the F1 regression (CSS selector happened to parse cleanly).
// With the ref-Map fix, backslash paths exercise the real production code path.
const projectPathA = "C:\\ProjectA";
const projectPathB = "C:\\ProjectB";

function projectDiscovery(path: string): AcDiscoveryResult {
  return discovery({
    workgroups: ["wg-1-dev-team", "wg-2-other-team"].map((name, index) => {
      const wgPath = `${path}\\.ac\\${name}`;
      return {
        name,
        path: wgPath,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: index === 0 ? "dev-webpage-ui" : "dev-rust",
            path: `${wgPath}\\__agent_${index === 0 ? "dev-webpage-ui" : "dev-rust"}`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      };
    }),
    agents: [],
    teams: [],
    loops: [],
  });
}

function groupsConfig(): WorkgroupGroupsConfig {
  const groupedWorkgroup = "wg-1-dev-team";
  return {
    ...defaultGroupsConfig(),
    groups: [
      {
        id: "dev",
        name: "Dev",
        regex: exactGroupRegexForWorkgroup(groupedWorkgroup),
      },
    ],
    nonStop: {
      ...defaultNonStop(),
      show: true,
      regex: exactGroupRegexForWorkgroup(groupedWorkgroup),
    },
  };
}

// Find the header by its raw title ATTRIBUTE value, not a CSS attribute
// selector: Windows backslashes are CSS string escapes (#810).
function findProjectHeader(root: ParentNode, projectPath: string): HTMLElement {
  const headers = Array.from(root.querySelectorAll<HTMLElement>(".project-header"));
  const header = headers.find((h) => h.getAttribute("title") === projectPath);
  if (!header) throw new Error(`project-header not found for ${projectPath}`);
  return header;
}

function findProjectHeaderButton(root: ParentNode, projectPath: string): HTMLElement {
  const header = findProjectHeader(root, projectPath);
  const btn = header.querySelector<HTMLElement>(".project-header-main");
  if (!btn) throw new Error(`project-header-main button not found for ${projectPath}`);
  return btn;
}

function headerCollapsed(header: HTMLElement): boolean {
  const chevron = header.querySelector(".ac-discovery-chevron");
  if (!chevron) throw new Error("Header chevron not found");
  return chevron.classList.contains("collapsed");
}

// Scope a rail button click to a specific project's rail section. The rail
// renders `data-ac-testid="workgroupGroups.rail.${folderName}"` per project.
function railButton(projectFolder: string, buttonKey: string): HTMLElement {
  const section = document.querySelector<HTMLElement>(
    `[data-ac-testid="workgroupGroups.rail.${projectFolder}"]`
  );
  if (!section) throw new Error(`rail section not found for ${projectFolder}`);
  const btn = section.querySelector<HTMLElement>(`[data-ac-testid="workgroupGroups.button.${buttonKey}"]`);
  if (!btn) throw new Error(`rail button ${buttonKey} not found in ${projectFolder}`);
  return btn;
}

function rect(top: number, height = 40): DOMRect {
  return {
    x: 0,
    y: top,
    width: 320,
    height,
    top,
    right: 320,
    bottom: top + height,
    left: 0,
    toJSON: () => ({}),
  };
}

function installScrollableLayout(
  root: ParentNode,
  projectTops: ReadonlyArray<readonly [string, number]>,
): HTMLDivElement {
  const scrollable = root.querySelector<HTMLDivElement>(".sidebar-scrollable");
  if (!scrollable) throw new Error("sidebar-scrollable not found");
  const viewportTop = 100;
  scrollable.getBoundingClientRect = () => rect(viewportTop, 240);

  for (const [projectPath, logicalTop] of projectTops) {
    const panel = findProjectHeader(root, projectPath).closest<HTMLElement>(".project-panel");
    if (!panel) throw new Error(`project-panel not found for ${projectPath}`);
    panel.getBoundingClientRect = () => rect(viewportTop + logicalTop - scrollable.scrollTop, 180);
  }
  return scrollable;
}

function installScrollTopWriteTrap(scrollable: HTMLDivElement, initialValue: number) {
  let currentValue = initialValue;
  const writes: number[] = [];
  Object.defineProperty(scrollable, "scrollTop", {
    configurable: true,
    get: () => currentValue,
    set: (nextValue: number) => {
      currentValue = nextValue;
      writes.push(nextValue);
    },
  });
  return { currentValue: () => currentValue, writes };
}

async function flushReactiveWork(): Promise<void> {
  // Solid schedules the effect after the store update, and production adds a
  // queueMicrotask inside it. Crossing a task boundary drains both layers.
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

// Component that renders the production rail/panel boundary and installs the
// same scroll-owner primitive that SidebarApp uses.
const SidebarHarness: Component = () => {
  let scrollableEl: HTMLDivElement | undefined;
  createSidebarSelectionScrollReset(() => scrollableEl);
  return (
    <div class="sidebar-body">
      <WorkgroupGroupRail projects={projectStore.projects} />
      <div class="sidebar-scrollable" ref={scrollableEl}>
        <ProjectPanel />
      </div>
    </div>
  );
};

describe("WorkgroupGroupRail selection focus (#810/#941)", () => {
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

  it("uses a stable primitive key for equal selections and distinct keys for real changes", () => {
    workgroupGroupsStore.applyExternalUpdate(projectPathA, groupsConfig());
    workgroupGroupsStore.applyExternalUpdate(projectPathB, groupsConfig());

    workgroupGroupsStore.select(projectPathA, { kind: "all" });
    const projectAAllKey = activeWorkgroupGroupSelectionKey();
    expect(typeof projectAAllKey).toBe("string");

    // Rebuild the same selection object: the semantic key stays primitive and
    // value-equal even though the store entry emitted a new object identity.
    workgroupGroupsStore.select(projectPathA, { kind: "all" });
    expect(activeWorkgroupGroupSelectionKey()).toBe(projectAAllKey);

    workgroupGroupsStore.select(projectPathA, { kind: "group", id: "dev" });
    const projectAGroupKey = activeWorkgroupGroupSelectionKey();
    expect(projectAGroupKey).not.toBe(projectAAllKey);

    workgroupGroupsStore.select(projectPathA, { kind: "all" });
    const projectAAllAgainKey = activeWorkgroupGroupSelectionKey();
    expect(projectAAllAgainKey).toBe(projectAAllKey);
    expect(projectAAllAgainKey).not.toBe(projectAGroupKey);

    workgroupGroupsStore.select(projectPathA, { kind: "ungrouped" });
    const projectAUngroupedKey = activeWorkgroupGroupSelectionKey();
    expect(projectAUngroupedKey).not.toBe(projectAAllAgainKey);

    workgroupGroupsStore.select(projectPathA, { kind: "nonstop" });
    const projectANonStopKey = activeWorkgroupGroupSelectionKey();
    expect(projectANonStopKey).not.toBe(projectAUngroupedKey);

    workgroupGroupsStore.select(projectPathB, { kind: "all" });
    const projectBAllKey = activeWorkgroupGroupSelectionKey();
    expect(projectBAllKey).not.toBe(projectANonStopKey);
    expect(projectBAllKey).not.toBe(projectAAllKey);
  });

  it("does not write scrollTop when an equal selection object is emitted", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path);
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await waitFor(() => expect(railButton("ProjectA", "all")).toBeTruthy());

      const scrollable = installScrollableLayout(rendered.root, [[projectPathA, 0]]);
      await flushReactiveWork();

      const priorScrollTop = 137;
      const scrollState = installScrollTopWriteTrap(scrollable, priorScrollTop);

      // This deliberately rebuilds the entry and selection objects while
      // preserving the exact active-project/selection semantics.
      workgroupGroupsStore.select(projectPathA, { kind: "all" });
      await flushReactiveWork();

      expect(scrollState.writes).toEqual([]);
      expect(scrollState.currentValue()).toBe(priorScrollTop);
    } finally {
      rendered.cleanup();
    }
  });

  it("resets every real selection kind and aligns a non-first project header", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path);
      if (path === projectPathB) return projectDiscovery(path);
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await projectStore.createAndLoad(projectPathB);
      await waitFor(() => {
        expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy();
        expect(findProjectHeader(rendered.root, projectPathB)).toBeTruthy();
        expect(railButton("ProjectA", "dev")).toBeTruthy();
        expect(railButton("ProjectA", "nonstop")).toBeTruthy();
      });

      const scrollable = installScrollableLayout(rendered.root, [
        [projectPathA, 0],
        [projectPathB, 240],
      ]);

      // All -> custom -> All -> Ungrouped -> Alert me! are all real semantic
      // changes and must discard a positive previous offset instantly.
      for (const [buttonKey, oldOffset] of [
        ["dev", 111],
        ["all", 112],
        ["ungrouped", 113],
        ["nonstop", 114],
      ] as const) {
        scrollable.scrollTop = oldOffset;
        click(railButton("ProjectA", buttonKey));
        await waitFor(() => expect(scrollable.scrollTop).toBe(0));
      }

      // ProjectB remembers All, but changing the active project is still a real
      // selection change. Its non-first panel must align to the scrollport top.
      scrollable.scrollTop = 47;
      click(railButton("ProjectB", "all"));
      await waitFor(() => expect(scrollable.scrollTop).toBe(240));
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(true);
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathB))).toBe(false);
      expect(projectCollapseStore.focusTarget()).toBe(null);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps scroll fixed on a same-selection re-click while preserving collapse and expand", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path);
      if (path === projectPathB) return projectDiscovery(path);
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await projectStore.createAndLoad(projectPathB);
      await waitFor(() => expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy());

      const scrollable = installScrollableLayout(rendered.root, [
        [projectPathA, 0],
        [projectPathB, 240],
      ]);

      // All is already the active selection. Manually collapse its owner first;
      // ProjectB remains expanded, so the re-click has both side effects to do.
      click(findProjectHeaderButton(rendered.root, projectPathA));
      await waitFor(() =>
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(true)
      );

      const priorScrollTop = 137;
      scrollable.scrollTop = priorScrollTop;
      click(railButton("ProjectA", "all"));
      // Assert the scroll position directly after both re-click side effects
      // settle; this behavior must not depend on the focus-target plumbing.
      await waitFor(() => {
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false);
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathB))).toBe(true);
        expect(scrollable.scrollTop).toBe(priorScrollTop);
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("preserves the other project's sub-section collapse when auto-focus collapses its project", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path);
      if (path === projectPathB) return projectDiscovery(path);
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await projectStore.createAndLoad(projectPathB);
      await waitFor(() => expect(findProjectHeader(rendered.root, projectPathB)).toBeTruthy());

      // Collapse the "Workgroups" sub-section header in ProjectB before auto-focus.
      const workgroupsHeaderB = Array.from(
        rendered.root.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible")
      ).find((h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === "Workgroups");
      if (!workgroupsHeaderB) throw new Error("Workgroups sub-header not found in ProjectB");
      // First locate ProjectB's panel to scope the sub-header lookup.
      const projectBPanel = findProjectHeader(rendered.root, projectPathB).closest(".project-panel");
      const workgroupsHeaderInB = Array.from(
        projectBPanel!.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible")
      ).find((h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === "Workgroups");
      if (!workgroupsHeaderInB) throw new Error("Workgroups sub-header not found inside ProjectB panel");
      click(workgroupsHeaderInB);
      await waitFor(() => {
        const chev = workgroupsHeaderInB.querySelector(".ac-discovery-chevron");
        expect(chev?.classList.contains("collapsed")).toBe(true);
      });

      // Auto-focus ProjectA -> ProjectB's project-level collapses, but its
      // "Workgroups" sub-section collapse choice must survive (sub-sections
      // are NOT touched by #810 - they live in the local collapsedByKey signal).
      click(railButton("ProjectA", "all"));
      await waitFor(() =>
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathB))).toBe(true)
      );

      // ProjectB is now project-collapsed, but its Workgroups sub-section
      // header chevron is still collapsed (preserved).
      expect(workgroupsHeaderInB.querySelector(".ac-discovery-chevron")?.classList.contains("collapsed")).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("resets an actual selection change in a single-project layout", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path);
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await waitFor(() => expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy());

      const scrollable = installScrollableLayout(rendered.root, [[projectPathA, 0]]);
      scrollable.scrollTop = 83;
      click(railButton("ProjectA", "dev"));
      await waitFor(() => expect(scrollable.scrollTop).toBe(0));
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
});
