// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcWorkgroup } from "../../shared/types";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { ProjectState } from "../stores/project";
import { defaultGroupsConfig } from "../stores/workgroup-groups";
import WorkgroupGroupRail from "./WorkgroupGroupRail";

const projectPath = "C:\\Project";

function wg(name: string): AcWorkgroup {
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle: null,
    agents: [],
  };
}

function project(): ProjectState {
  return {
    path: projectPath,
    folderName: "Project",
    workgroups: [wg("wg-1-dev-team")],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
  };
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

describe("WorkgroupGroupRail archive entry point (#881)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("renders one archive button with zero projects and opens the archived projects modal", async () => {
    const fake = new FakeTransport();
    fake.resolve("list_archived_projects", []);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[]} />, fake);
    try {
      expect(document.querySelectorAll('[data-ac-testid="workgroupGroups.rail.archive"]')).toHaveLength(1);
      expect(document.querySelectorAll('[data-ac-testid^="workgroupGroups.button."]')).toHaveLength(0);

      click(byTestId("workgroupGroups.rail.archive"));
      await waitFor(() => expect(byTestId("archivedProjects.modal")).toBeTruthy());
      expect(byTestId("archivedProjects.empty").textContent).toContain("No archived projects");
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps project sections inside the scroll wrapper and the archive button outside it", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_project_groups", defaultGroupsConfig());
    fake.resolve("list_archived_projects", []);

    const rendered = renderWithFakeTransport(() => <WorkgroupGroupRail projects={[project()]} />, fake);
    try {
      await waitFor(() => expect(byTestId("workgroupGroups.rail.Project")).toBeTruthy());

      const rail = byTestId("workgroupGroups.rail");
      const scroll = rail.querySelector(".workgroup-group-rail-scroll");
      const projectSection = byTestId("workgroupGroups.rail.Project");
      const archiveButton = byTestId("workgroupGroups.rail.archive");

      expect(scroll).toBeTruthy();
      expect(projectSection.parentElement).toBe(scroll);
      expect(archiveButton.parentElement).toBe(rail);
      expect(scroll?.contains(archiveButton)).toBe(false);
      expect(document.querySelectorAll('[data-ac-testid="workgroupGroups.rail.archive"]')).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });
});
