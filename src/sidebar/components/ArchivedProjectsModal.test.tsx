// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { ArchivedProject } from "../../shared/types";
import { projectStore } from "../stores/project";
import ArchivedProjectsModal from "./ArchivedProjectsModal";
import { automationIdPart } from "./replica-repo-badges";

function row(path: string, overrides: Partial<ArchivedProject> = {}): ArchivedProject {
  return {
    path,
    folderName: path.replace(/\\/g, "/").split("/").pop() ?? path,
    exists: true,
    hasWorkspace: true,
    ...overrides,
  };
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

describe("ArchivedProjectsModal (#881)", () => {
  beforeEach(() => {
    resetUiStoresForTests();
  });

  afterEach(() => {
    resetUiStoresForTests();
    projectStore.clear();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("renders the empty state", async () => {
    const fake = new FakeTransport();
    fake.resolve("list_archived_projects", []);

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      await waitFor(() => expect(byTestId("archivedProjects.empty")).toBeTruthy());
      expect(byTestId("archivedProjects.empty").textContent).toContain("No archived projects");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders rows, unarchives through the backend, discovers, and refetches", async () => {
    let rows = [row("C:\\Archive\\One"), row("C:\\Archive\\Two")];
    const fake = new FakeTransport();
    fake.onInvoke("list_archived_projects", () => rows);
    fake.onInvoke("unarchive_project", (args) => {
      rows = rows.filter((entry) => entry.path !== args.path);
      return { path: args.path as string, registered: true, created: false };
    });
    fake.resolve("discover_project", discovery());

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      const firstId = automationIdPart("C:\\Archive\\One");
      const secondId = automationIdPart("C:\\Archive\\Two");
      await waitFor(() => expect(byTestId(`archivedProjects.row.${firstId}`)).toBeTruthy());
      expect(byTestId(`archivedProjects.row.${secondId}`)).toBeTruthy();

      click(byTestId(`archivedProjects.unarchive.${firstId}`));

      await waitFor(() =>
        expect(document.querySelector(`[data-ac-testid="archivedProjects.row.${firstId}"]`)).toBeNull(),
      );
      expect(fake.callsFor("unarchive_project")[0].args.path).toBe("C:\\Archive\\One");
      expect(fake.callsFor("discover_project")[0].args.path).toBe("C:\\Archive\\One");
      expect(byTestId(`archivedProjects.row.${secondId}`)).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("derives row automation ids from the full path when folder names collide", async () => {
    const firstPath = "C:\\Archive\\A\\Same";
    const secondPath = "D:\\Archive\\B\\Same";
    const fake = new FakeTransport();
    fake.resolve("list_archived_projects", [
      row(firstPath, { folderName: "Same" }),
      row(secondPath, { folderName: "Same" }),
    ]);

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      await waitFor(() => expect(byTestId(`archivedProjects.row.${automationIdPart(firstPath)}`)).toBeTruthy());
      expect(byTestId(`archivedProjects.row.${automationIdPart(secondPath)}`)).toBeTruthy();
      expect(document.querySelectorAll('[data-ac-testid^="archivedProjects.row."]')).toHaveLength(2);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows Remove from list for missing folders and routes it through remove_project", async () => {
    let rows = [row("C:\\Archive\\Missing", { exists: false })];
    const fake = new FakeTransport();
    fake.onInvoke("list_archived_projects", () => rows);
    fake.onInvoke("remove_project", (args) => {
      rows = rows.filter((entry) => entry.path !== args.path);
      return undefined;
    });

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      const id = automationIdPart("C:\\Archive\\Missing");
      await waitFor(() => expect(byTestId(`archivedProjects.forget.${id}`)).toBeTruthy());

      click(byTestId(`archivedProjects.forget.${id}`));

      await waitFor(() => expect(byTestId("archivedProjects.empty")).toBeTruthy());
      expect(fake.callsFor("remove_project")[0].args.path).toBe("C:\\Archive\\Missing");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders a rejected unarchive under its own row and keeps the row present", async () => {
    const fake = new FakeTransport();
    fake.resolve("list_archived_projects", [row("C:\\Archive\\Blocked")]);
    fake.reject("unarchive_project", "Workspace missing");

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      const id = automationIdPart("C:\\Archive\\Blocked");
      await waitFor(() => expect(byTestId(`archivedProjects.unarchive.${id}`)).toBeTruthy());

      click(byTestId(`archivedProjects.unarchive.${id}`));

      await waitFor(() =>
        expect(byTestId(`archivedProjects.rowError.${id}`).textContent).toContain("Workspace missing"),
      );
      expect(byTestId(`archivedProjects.row.${id}`)).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("renders listArchived failures as a top-level alert, not an empty list", async () => {
    const fake = new FakeTransport();
    fake.reject("list_archived_projects", "settings locked");

    const rendered = renderWithFakeTransport(() => <ArchivedProjectsModal onClose={vi.fn()} />, fake);
    try {
      await waitFor(() => expect(byTestId("archivedProjects.error")).toBeTruthy());
      expect(byTestId("archivedProjects.error").textContent).toContain("settings locked");
      expect(document.querySelector('[data-ac-testid="archivedProjects.empty"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
