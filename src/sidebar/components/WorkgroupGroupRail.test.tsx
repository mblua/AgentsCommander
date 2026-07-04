// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup, Session, WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  input,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { ProjectState } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import {
  defaultGroupsConfig,
  defaultNonStop,
  exactGroupRegexForWorkgroup,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";
import WorkgroupGroupRail from "./WorkgroupGroupRail";

const projectPath = "C:\\Project";

function wg(name: string): AcWorkgroup {
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle: null,
    agents: [
      {
        name: "dev-webpage-ui",
        path: `${projectPath}\\.ac\\${name}\\__agent_dev-webpage-ui`,
        repoPaths: [],
        isCoordinator: true,
      },
    ],
  };
}

function project(): ProjectState {
  return {
    path: projectPath,
    folderName: "Project",
    workgroups: [wg("wg-1-dev-team"), wg("wg-2-rust-team"), wg("wg-3-docs-team")],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

function groupsConfig(overrides: Partial<WorkgroupGroupsConfig> = {}): WorkgroupGroupsConfig {
  return {
    ...defaultGroupsConfig(),
    ...overrides,
    groups:
      overrides.groups ?? [
        { id: "ui", name: "UI", regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
        { id: "rust", name: "Rust", regex: exactGroupRegexForWorkgroup("wg-2-rust-team") },
      ],
  };
}

function target<T extends Element>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing element ${testId}`);
  return element;
}

function changeCheckbox(input: HTMLInputElement, checked: boolean): void {
  input.checked = checked;
  input.dispatchEvent(new Event("change", { bubbles: true }));
}

function railButtonOrder(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.button."]')
  ).map((button) => button.dataset.acTestid!.replace("workgroupGroups.button.", ""));
}

function railDots(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="workgroupGroups.dot."]')
  ).map((dot) => dot.dataset.acTestid!.replace("workgroupGroups.dot.", ""));
}

/** A session that makes the given workgroup "working" (running dot class). */
function replicaSession(wgName: string, overrides: Partial<Session> = {}): Session {
  return session({
    id: `session-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: `${projectPath}\\.ac\\${wgName}\\__agent_dev-webpage-ui`,
    status: "running",
    ...overrides,
  });
}

describe("WorkgroupGroupRail", () => {
  beforeEach(() => {
    resetUiStoresForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("keeps All, All plus Ungrouped, and Ungrouped-only reachable while blocking none", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    fake.onInvoke("update_project_groups", (args) => args.config);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));

      click(target("workgroupGroups.edit"));
      changeCheckbox(target<HTMLInputElement>("workgroupGroups.toggle.showUngrouped"), false);
      click(target("workgroupGroups.save"));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          showAll: true,
          showUngrouped: false,
        })
      );
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ui", "rust"]));

      click(target("workgroupGroups.edit"));
      changeCheckbox(target<HTMLInputElement>("workgroupGroups.toggle.showUngrouped"), true);
      changeCheckbox(target<HTMLInputElement>("workgroupGroups.toggle.showAll"), false);
      click(target("workgroupGroups.save"));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          showAll: false,
          showUngrouped: true,
        })
      );
      await waitFor(() => expect(railButtonOrder()).toEqual(["ungrouped", "ui", "rust"]));

      click(target("workgroupGroups.edit"));
      changeCheckbox(target<HTMLInputElement>("workgroupGroups.toggle.showUngrouped"), false);
      await waitFor(() =>
        expect(target<HTMLInputElement>("workgroupGroups.toggle.showUngrouped").checked).toBe(true)
      );
      click(target("workgroupGroups.save"));

      await waitFor(() =>
        expect(fake.lastCall("update_project_groups")?.args.config).toMatchObject({
          showAll: false,
          showUngrouped: true,
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("renders Ungrouped immediately after All and first when All is hidden", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    fake.onInvoke("update_project_groups", (args) => args.config);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));

      await workgroupGroupsStore.save(
        projectPath,
        groupsConfig({
          showAll: false,
          showUngrouped: true,
        })
      );

      await waitFor(() => expect(railButtonOrder()).toEqual(["ungrouped", "ui", "rust"]));
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the working dot on All and on the group whose workgroup is running (#746)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([replicaSession("wg-1-dev-team")]);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      await waitFor(() => expect(railDots()).toEqual(["all", "ui"]));

      // Same visual language as session/replica rows: the standard blue
      // running dot, rendered on the counter line.
      const dot = target<HTMLElement>("workgroupGroups.dot.ui");
      expect(dot.classList.contains("session-item-status")).toBe(true);
      expect(dot.classList.contains("running")).toBe(true);
      // Dot condition matches the counter's X definition.
      expect(target("workgroupGroups.button.all").textContent).toContain("1/3");
      expect(target("workgroupGroups.button.ui").textContent).toContain("1/1");
      expect(target("workgroupGroups.button.rust").textContent).toContain("0/1");
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the working dot on Ungrouped when an ungrouped workgroup is running (#746)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([replicaSession("wg-3-docs-team")]);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      await waitFor(() => expect(railDots()).toEqual(["all", "ungrouped"]));
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps every dot off for waiting/pending agents, matching the counter (#746)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());
    sessionsStore.setSessions([
      replicaSession("wg-1-dev-team", { waitingForInput: true }),
      replicaSession("wg-2-rust-team", { pendingReview: true }),
    ]);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      expect(railDots()).toEqual([]);
      expect(target("workgroupGroups.button.all").textContent).toContain("0/3");
    } finally {
      rendered.cleanup();
    }
  });

  it("focuses and selects the new row's name input after Add group (#746)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig());

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));

      click(target("workgroupGroups.edit"));
      click(target("workgroupGroups.add"));

      let added!: HTMLInputElement;
      await waitFor(() => {
        const inputs = document.querySelectorAll<HTMLInputElement>(".workgroup-group-name-input");
        added = inputs[inputs.length - 1];
        expect(added.value).toBe("Group 3");
        expect(document.activeElement).toBe(added);
        // Pre-filled name is selected so typing replaces it.
        expect(added.selectionStart).toBe(0);
        expect(added.selectionEnd).toBe("Group 3".length);
      });

      // Typing must not drop focus: <Index> keeps the row's DOM stable while
      // updateGroup replaces the row object per keystroke (the #614 trap).
      input(added, "B");
      input(added, "Ba");
      await waitFor(() => {
        expect(added.isConnected).toBe(true);
        expect(document.activeElement).toBe(added);
        expect(added.value).toBe("Ba");
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("#777 pins the Non-stop button after Ungrouped with the live counter + running dot", async () => {
    const fake = new FakeTransport();
    fake.resolve(
      "get_project_groups",
      groupsConfig({
        nonStop: { ...defaultNonStop(), show: true, regex: exactGroupRegexForWorkgroup("wg-1-dev-team") },
      })
    );
    fake.onInvoke("update_project_groups", (args) => args.config);
    // wg-1 is working (running session) -> counter 1/1 + dot.
    sessionsStore.setSessions([replicaSession("wg-1-dev-team")]);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() =>
        expect(railButtonOrder()).toEqual(["all", "ungrouped", "nonstop", "ui", "rust"])
      );
      expect(target("workgroupGroups.button.nonstop").textContent).toContain("1/1");
      expect(railDots()).toContain("nonstop");
    } finally {
      rendered.cleanup();
    }
  });

  it("#777 hides the Non-stop button when show=false", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", groupsConfig({ nonStop: { ...defaultNonStop(), show: false } }));
    fake.onInvoke("update_project_groups", (args) => args.config);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(railButtonOrder()).toEqual(["all", "ungrouped", "ui", "rust"]));
      expect(document.querySelector('[data-ac-testid="workgroupGroups.button.nonstop"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
