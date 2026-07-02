// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup, WorkgroupGroupsConfig } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { ProjectState } from "../stores/project";
import {
  defaultGroupsConfig,
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
});
