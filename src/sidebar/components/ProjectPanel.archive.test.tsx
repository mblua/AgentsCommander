// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ToastHost from "../../shared/components/ToastHost";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import ProjectPanel from "./ProjectPanel";
import { automationIdPart } from "./replica-repo-badges";

const projectPath = "C:\\Project";

function setupTransport(fake: FakeTransport): void {
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("discover_project", discovery());
  fake.resolve("get_settings", baseSettings());
}

function projectHeader(): HTMLElement {
  const header = document.querySelector<HTMLElement>(".project-header");
  if (!header) throw new Error("project header not found");
  return header;
}

function menuButtons(): HTMLButtonElement[] {
  const menu = document.querySelector<HTMLElement>(".session-context-menu");
  if (!menu) throw new Error("context menu not found");
  return Array.from(menu.querySelectorAll<HTMLButtonElement>(".session-context-option"));
}

function buttonByText(label: string): HTMLButtonElement {
  const button = menuButtons().find((candidate) => candidate.textContent?.trim() === label);
  if (!button) throw new Error(`button not found: ${label}`);
  return button;
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T {
  const element = document.querySelector<T>(`[data-ac-testid="${testId}"]`);
  if (!element) throw new Error(`Missing ${testId}`);
  return element;
}

describe("ProjectPanel Archive Project action (#881)", () => {
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
    vi.restoreAllMocks();
  });

  it("renders Archive Project above Remove Project without danger styling and routes clicks to archive_project", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.resolve("archive_project", undefined);

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(projectHeader()).toBeTruthy());

      contextMenu(projectHeader());
      await waitFor(() => expect(buttonByText("Archive Project")).toBeTruthy());

      const labels = menuButtons().map((button) => button.textContent?.trim());
      expect(labels.indexOf("Archive Project")).toBeGreaterThan(-1);
      expect(labels.indexOf("Archive Project")).toBeLessThan(labels.indexOf("Remove Project"));
      expect(buttonByText("Archive Project").classList.contains("context-option-danger")).toBe(false);
      expect(buttonByText("Remove Project").classList.contains("context-option-danger")).toBe(true);

      click(byTestId(`project.action.archive.${automationIdPart(projectPath)}`));

      await waitFor(() => expect(fake.callsFor("archive_project")).toHaveLength(1));
      expect(fake.callsFor("archive_project")[0].args.path).toBe(projectPath);
      expect(projectStore.projects).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the backend rejection and leaves the project rendered", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    fake.reject("archive_project", "Cannot archive: 1 open session in this project.");

    const rendered = renderWithFakeTransport(
      () => (
        <>
          <ProjectPanel />
          <ToastHost />
        </>
      ),
      fake,
    );
    try {
      await projectStore.createAndLoad(projectPath);
      await waitFor(() => expect(projectHeader()).toBeTruthy());

      contextMenu(projectHeader());
      await waitFor(() => expect(buttonByText("Archive Project")).toBeTruthy());
      click(buttonByText("Archive Project"));

      await waitFor(() =>
        expect(byTestId("toast.item").textContent).toContain(
          "Cannot archive: 1 open session in this project.",
        ),
      );
      expect(projectHeader()).toBeTruthy();
      expect(projectStore.projects).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });
});
