// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  input,
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

const projectPath = "C:\\Project";
const wg1Path = `${projectPath}\\.ac\\wg-1-dev-team`;
const wg2Path = `${projectPath}\\.ac\\wg-2-rust-team`;
const coordRowTestId = "replica.row.quick.wg-1-dev-team.dev-webpage-ui";

function groupsConfig(groups = [{ id: "frontend", name: "Frontend", regex: "^wg-1-" }]) {
  return {
    ...defaultGroupsConfig(),
    groups,
  };
}

function projectDiscovery() {
  return discovery({
    workgroups: [
      {
        name: "wg-1-dev-team",
        path: wg1Path,
        task: null,
        taskTitle: "Frontend work",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${wg1Path}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
          {
            name: "dev-rust",
            path: `${wg1Path}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: false,
          },
        ],
      },
      {
        name: "wg-2-rust-team",
        path: wg2Path,
        task: null,
        taskTitle: "Rust work",
        agents: [
          {
            name: "dev-rust",
            path: `${wg2Path}\\__agent_dev-rust`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
  });
}

function setupProjectTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", projectDiscovery());
  fake.onInvoke("update_project_groups", (args) => args.config);
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function mouse(el: Element, type: "mouseenter" | "mouseleave"): void {
  el.dispatchEvent(new MouseEvent(type, { bubbles: false, cancelable: true }));
}

function focus(el: Element): void {
  el.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
  el.dispatchEvent(new FocusEvent("focus", { bubbles: false }));
}

function keydown(el: Element, key: string): void {
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

function panelTarget<T extends Element>(root: ParentNode, testId: string): T {
  const element = root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

async function openCoordinatorMenu(root: ParentNode): Promise<void> {
  contextMenu(panelTarget(root, coordRowTestId));
  await waitFor(() => expect(document.querySelector(".session-context-menu")).not.toBeNull());
}

async function openGroupFlyout(): Promise<void> {
  click(target("replica.wg-1-dev-team.groups.trigger"));
  await waitFor(() =>
    expect(target("replica.wg-1-dev-team.groups.flyout")).not.toBeNull()
  );
}

describe("ProjectPanel workgroup groups", () => {
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

  it("ANDs the selected group with the existing visible regex filter", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig());
      workgroupGroupsStore.select(projectPath, { kind: "group", id: "frontend" });
      await projectStore.createAndLoad(projectPath);

      await waitFor(() => {
        expect(rendered.root.textContent).toContain("wg-1-dev-team");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });

      const toggle = panelTarget<HTMLButtonElement>(rendered.root, "project.regexFilter.toggle");
      click(toggle);
      const filterInput = panelTarget<HTMLInputElement>(rendered.root, "project.regexFilter.input");

      input(filterInput, "rust-team");
      await waitFor(() => {
        expect(rendered.root.textContent).not.toContain("wg-1-dev-team");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });

      input(filterInput, "webpage");
      await waitFor(() => {
        expect(rendered.root.textContent).toContain("dev-webpage-ui");
        expect(rendered.root.textContent).not.toContain("dev-rust");
        expect(rendered.root.textContent).not.toContain("wg-2-rust-team");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("adds a coordinator workgroup to an existing group from the context menu", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${wg1Path}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig([{ id: "frontend", name: "Frontend", regex: "(?!)" }])
      );
      fake.clearCalls();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      expect(document.querySelector(".session-context-menu")?.textContent).toContain("Add to Group");
      expect(document.querySelector(".session-context-menu")?.textContent).not.toContain("Create new group");
      await openGroupFlyout();
      click(target(`replica.wg-1-dev-team.groups.frontend`));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          groups: [
            {
              id: "frontend",
              regex: exactGroupRegexForWorkgroup("wg-1-dev-team"),
            },
          ],
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("removes a coordinator workgroup from an exact generated group from the context menu", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${wg1Path}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig([
          {
            id: "frontend",
            name: "Frontend",
            regex: "^(wg-1-dev-team|wg-2-rust-team)$",
          },
        ])
      );
      fake.clearCalls();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      expect(target("replica.wg-1-dev-team.groups.frontend").textContent).toContain("\u2713");
      click(target("replica.wg-1-dev-team.groups.frontend"));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          groups: [
            {
              id: "frontend",
              regex: exactGroupRegexForWorkgroup("wg-2-rust-team"),
            },
          ],
        })
      );
      await waitFor(() =>
        expect(target("replica.wg-1-dev-team.groups.frontend").textContent).not.toContain("\u2713")
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps custom regex membership checked but non-toggleable in the group flyout", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${wg1Path}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig([{ id: "custom", name: "Custom", regex: "^wg-1-" }])
      );
      fake.clearCalls();
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      const option = target<HTMLButtonElement>("replica.wg-1-dev-team.groups.custom");
      expect(option.textContent).toContain("\u2713");
      expect(option.disabled).toBe(true);
      expect(option.title).toContain("custom regex");
      click(option);
      expect(fake.callsFor("update_project_groups")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the context menu and flyout open when group removal fails", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    sessionsStore.setSessions([
      session({
        id: "coord-session",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: `${wg1Path}\\__agent_dev-webpage-ui`,
        status: "running",
        isCoordinator: true,
      }),
    ]);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig([
          {
            id: "frontend",
            name: "Frontend",
            regex: exactGroupRegexForWorkgroup("wg-1-dev-team"),
          },
        ])
      );
      fake.reject("update_project_groups", "groups.toml changed on disk");
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      click(target("replica.wg-1-dev-team.groups.frontend"));

      await waitFor(() => {
        expect(document.querySelector(".session-context-menu")).not.toBeNull();
        expect(target("replica.wg-1-dev-team.groups.flyout")).not.toBeNull();
        expect(target("replica.groups.error").textContent).toContain("groups.toml changed on disk");
      });

      vi.useFakeTimers();
      try {
        mouse(target("replica.wg-1-dev-team.groups.flyout"), "mouseleave");
        vi.advanceTimersByTime(240);
        await Promise.resolve();
        expect(target("replica.wg-1-dev-team.groups.flyout")).not.toBeNull();
      } finally {
        vi.useRealTimers();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("auto-hides the Add to Group flyout after the pointer leaves the trigger and flyout", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig([]));
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      const trigger = target("replica.wg-1-dev-team.groups.trigger");
      const flyout = target("replica.wg-1-dev-team.groups.flyout");

      vi.useFakeTimers();
      try {
        mouse(trigger, "mouseleave");
        vi.advanceTimersByTime(120);
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull();

        mouse(flyout, "mouseenter");
        vi.advanceTimersByTime(240);
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull();

        mouse(flyout, "mouseleave");
        vi.advanceTimersByTime(180);
        await Promise.resolve();
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).toBeNull();
      } finally {
        vi.useRealTimers();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the Add to Group flyout open when pointer click follows hover open", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig([]));
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      const trigger = target("replica.wg-1-dev-team.groups.trigger");
      mouse(trigger, "mouseenter");
      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull()
      );

      click(trigger);

      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull()
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the Add to Group flyout open when keyboard activation follows focus open", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig([]));
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      const trigger = target("replica.wg-1-dev-team.groups.trigger");
      focus(trigger);
      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull()
      );

      keydown(trigger, "Enter");
      click(trigger);

      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="replica.wg-1-dev-team.groups.flyout"]')).not.toBeNull()
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("focuses the inline create-group input when it appears (#746)", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    fake.resolve("get_project_groups", groupsConfig([]));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.ensureLoaded(projectPath);
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      click(target("replica.wg-1-dev-team.groups.create"));

      // Resolve the input inside waitFor: a discovery-driven row re-render can
      // re-create the flyout subtree once after the click, and the fresh
      // input's ref re-focuses it — assert the live element ends up focused.
      await waitFor(() =>
        expect(document.activeElement).toBe(target<HTMLInputElement>("replica.groups.create.input"))
      );
      // The flyout stays open and expanded around the focused input.
      expect(target("replica.wg-1-dev-team.groups.flyout")).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the context menu open and shows an error when inline group creation fails", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);
    fake.resolve("get_project_groups", groupsConfig([]));

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await workgroupGroupsStore.ensureLoaded(projectPath);
      fake.reject("update_project_groups", "groups.toml changed on disk");
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      await openGroupFlyout();
      expect(target("replica.wg-1-dev-team.groups.flyout").textContent).toContain("No groups yet");
      expect(target("replica.wg-1-dev-team.groups.flyout").textContent).toContain("Create new group");
      click(target("replica.wg-1-dev-team.groups.create"));
      input(target<HTMLInputElement>("replica.groups.create.input"), "Frontend");
      click(target("replica.groups.create.save"));

      await waitFor(() => {
        expect(document.querySelector(".session-context-menu")).not.toBeNull();
        expect(target("replica.groups.error").textContent).toContain("groups.toml changed on disk");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("clamps the Add to Group flyout inside the viewport near the right and bottom edges", async () => {
    const fake = new FakeTransport();
    setupProjectTransport(fake);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    const originalWidth = window.innerWidth;
    const originalHeight = window.innerHeight;
    try {
      await workgroupGroupsStore.save(projectPath, groupsConfig([]));
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(rendered.root.textContent).toContain("dev-webpage-ui"));

      await openCoordinatorMenu(rendered.root);
      Object.defineProperty(window, "innerWidth", { configurable: true, value: 320 });
      Object.defineProperty(window, "innerHeight", { configurable: true, value: 220 });
      const trigger = target<HTMLButtonElement>("replica.wg-1-dev-team.groups.trigger");
      trigger.getBoundingClientRect = () =>
        ({
          left: 295,
          right: 315,
          top: 185,
          bottom: 205,
          width: 20,
          height: 20,
          x: 295,
          y: 185,
          toJSON: () => ({}),
        }) as DOMRect;

      click(trigger);
      const flyout = target<HTMLDivElement>("replica.wg-1-dev-team.groups.flyout");
      flyout.getBoundingClientRect = () =>
        ({
          left: 0,
          right: 220,
          top: 0,
          bottom: 180,
          width: 220,
          height: 180,
          x: 0,
          y: 0,
          toJSON: () => ({}),
        }) as DOMRect;
      mouse(trigger, "mouseenter");

      await waitFor(() => {
        const left = Number.parseFloat(flyout.style.left);
        const top = Number.parseFloat(flyout.style.top);
        expect(left).toBeGreaterThanOrEqual(8);
        expect(left + 220).toBeLessThanOrEqual(312);
        expect(top).toBeGreaterThanOrEqual(8);
        expect(top + 180).toBeLessThanOrEqual(212);
      });
    } finally {
      Object.defineProperty(window, "innerWidth", { configurable: true, value: originalWidth });
      Object.defineProperty(window, "innerHeight", { configurable: true, value: originalHeight });
      rendered.cleanup();
    }
  });
});
