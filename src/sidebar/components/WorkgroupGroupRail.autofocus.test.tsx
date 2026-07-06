// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
import { defaultGroupsConfig, workgroupGroupsStore } from "../stores/workgroup-groups";
import { projectCollapseStore } from "../stores/project-collapse";
import WorkgroupGroupRail from "./WorkgroupGroupRail";
import ProjectPanel from "./ProjectPanel";

// #810 - Two backslash Windows paths (grinch F4). Forward-slash or simplified
// paths would hide the F1 regression (CSS selector happened to parse cleanly).
// With the ref-Map fix, backslash paths exercise the real production code path.
const projectPathA = "C:\\ProjectA";
const projectPathB = "C:\\ProjectB";

function projectDiscovery(path: string, folderName: string): AcDiscoveryResult {
  const wgPath = `${path}\\.ac\\wg-1-dev-team`;
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wgPath,
        task: null,
        taskTitle: null,
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${wgPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
    agents: [],
    teams: [],
    loops: [],
  });
}

function groupsConfig(): WorkgroupGroupsConfig {
  return { ...defaultGroupsConfig() };
}

// #810 (grinch F1) - find the .project-header container by its title
// ATTRIBUTE value via getAttribute, NOT via a CSS attribute selector. A CSS
// selector like `[title="C:\ProjectA"]` would parse `\P` as `P` (CSS
// string-token escape) and fail to match the real attribute - the exact bug
// F1 fixed in the production code. The test must NOT use the broken pattern.
// After main's restructure, .project-header is a <div> container holding a
// <button class="project-header-main">. The ref-Map stores the .project-header
// div (that's where the ref callback lives), so scrollIntoView is called on
// it. The click-to-toggle target is the .project-header-main button inside.
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

// Component that renders both siblings the same way App.tsx does (rail +
// scrollable ProjectPanel), so the rail's onClick can drive the shared
// projectCollapseStore that ProjectPanel reads.
const SidebarHarness: Component<{ projects: typeof projectStore.projects }> = () => {
  return (
    <div class="sidebar-body">
      <WorkgroupGroupRail projects={projectStore.projects} />
      <div class="sidebar-scrollable">
        <ProjectPanel />
      </div>
    </div>
  );
};

describe("WorkgroupGroupRail autofocus (#810)", () => {
  let cleanupDom: (() => void) | null = null;
  let scrollIntoViewMock: ReturnType<typeof vi.fn>;
  let scrollCalls: HTMLElement[] = [];

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    scrollCalls = [];
    // #810 - jsdom throws "Not implemented: window.scrollIntoView" without a
    // mock. Capture the `this` element of each call so we can assert the
    // scroll targeted the owner's specific header element (grinch F4).
    scrollIntoViewMock = vi.fn(function (this: HTMLElement, _options: ScrollIntoViewOptions) {
      scrollCalls.push(this);
    });
    Element.prototype.scrollIntoView = scrollIntoViewMock as any;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("collapses other projects, expands the owner, and scrolls the owner header into view", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path, "ProjectA");
      if (path === projectPathB) return projectDiscovery(path, "ProjectB");
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness projects={projectStore.projects} />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await projectStore.createAndLoad(projectPathB);
      await waitFor(() => {
        expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy();
        expect(findProjectHeader(rendered.root, projectPathB)).toBeTruthy();
      });

      // Both start expanded (default).
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false);
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathB))).toBe(false);

      // Click the "All" group button in ProjectA's rail section.
      click(railButton("ProjectA", "all"));

      // Owner (A) stays/becomes expanded; other (B) is collapsed.
      await waitFor(() => {
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false);
      });
      await waitFor(() => {
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathB))).toBe(true);
      });

      // scrollIntoView fired on ProjectA's specific .project-header element
      // (the one whose title attr equals ProjectA's path), with the mandated
      // options. This is the F1 regression guard: a CSS-selector-based lookup
      // would have returned null on the backslash path and no-op'd.
      await waitFor(() => expect(scrollCalls.length).toBeGreaterThanOrEqual(1));
      const ownerHeader = findProjectHeader(rendered.root, projectPathA);
      const ownerScrollCall = scrollCalls.find((el) => el === ownerHeader);
      expect(ownerScrollCall).toBeTruthy();
      expect(scrollIntoViewMock).toHaveBeenCalledWith({ block: "nearest", behavior: "smooth" });

      // Focus target is consumed (one-shot).
      expect(projectCollapseStore.focusTarget()).toBe(null);
    } finally {
      rendered.cleanup();
    }
  });

  it("expands the owner even if it was manually collapsed beforehand (locked decision: expand owner always)", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path, "ProjectA");
      if (path === projectPathB) return projectDiscovery(path, "ProjectB");
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness projects={projectStore.projects} />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await projectStore.createAndLoad(projectPathB);
      await waitFor(() => expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy());

      // Manually collapse ProjectA first (click the .project-header-main
      // button, which is the actual toggle target after main's restructure).
      click(findProjectHeaderButton(rendered.root, projectPathA));
      await waitFor(() =>
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(true)
      );

      // Auto-focus from the rail must override the manual collapse.
      click(railButton("ProjectA", "all"));
      await waitFor(() =>
        expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false)
      );
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
      if (path === projectPathA) return projectDiscovery(path, "ProjectA");
      if (path === projectPathB) return projectDiscovery(path, "ProjectB");
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness projects={projectStore.projects} />, fake);
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

  it("does not throw on a single-project layout and still scrolls the owner into view", async () => {
    const fake = new FakeTransport();
    fake.onInvoke("new_project", (args) => ({
      path: args.path as string,
      registered: true,
      created: false,
    }));
    fake.onInvoke("discover_project", (args) => {
      const path = args.path as string;
      if (path === projectPathA) return projectDiscovery(path, "ProjectA");
      throw new Error(`unexpected discover_project path: ${path}`);
    });
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <SidebarHarness projects={projectStore.projects} />, fake);
    try {
      await projectStore.createAndLoad(projectPathA);
      await waitFor(() => expect(findProjectHeader(rendered.root, projectPathA)).toBeTruthy());

      // Single project: collapse-others is a no-op (only owner in the list),
      // but the click must not throw and the owner still scrolls into view.
      click(railButton("ProjectA", "all"));
      await waitFor(() => expect(scrollCalls.length).toBeGreaterThanOrEqual(1));
      const ownerHeader = findProjectHeader(rendered.root, projectPathA);
      expect(scrollCalls.find((el) => el === ownerHeader)).toBeTruthy();
      expect(headerCollapsed(findProjectHeader(rendered.root, projectPathA))).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
});
