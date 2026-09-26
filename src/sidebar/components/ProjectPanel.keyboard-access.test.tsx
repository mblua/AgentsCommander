// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import type { AcDiscoveryResult } from "../../shared/types";
import { loopSummaryFixture as loop } from "./loop-summary-fixture";

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;

function projectDiscovery(): AcDiscoveryResult {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Task",
        agents: [
          {
            name: "dev-webpage-ui",
            path: `${workgroupPath}\\__agent_dev-webpage-ui`,
            repoPaths: [],
            isCoordinator: true,
          },
        ],
      },
    ],
    loops: [loop()],
  });
}

function headerByName(root: ParentNode, name: string): HTMLElement {
  const header = Array.from(root.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible")).find(
    (candidate) => candidate.querySelector(".ac-wg-name")?.textContent?.trim() === name
  );
  if (!header) throw new Error(`Header not found: ${name}`);
  return header;
}

function collapsed(root: ParentNode, name: string): boolean {
  return headerByName(root, name).querySelector(".ac-discovery-chevron")!.classList.contains("collapsed");
}

function pressOnProxy(root: ParentNode, name: string, key: string): void {
  const proxy = headerByName(root, name).firstElementChild!;
  expect(proxy.classList.contains("ac-row-key-proxy")).toBe(true);
  proxy.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}

describe("ProjectPanel header keyboard access (#2657)", () => {
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

  it("toggles each collapsible header with Enter, Space and mouse click", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: projectPath, registered: true, created: false });
    fake.resolve("discover_project", projectDiscovery());

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    try {
      await projectStore.createAndLoad(projectPath);
      const quickRow = '[data-ac-testid="replica.row.quick.wg-2-dev-team.dev-webpage-ui"]';
      await waitFor(() => expect(rendered.root.querySelector(quickRow)).not.toBeNull());
      // An active orchestrator session makes the Selected Room header render.
      sessionsStore.setSessions([
        session({
          id: "coord",
          name: "wg-2-dev-team/dev-webpage-ui",
          workingDirectory: `${workgroupPath}\\__agent_dev-webpage-ui`,
          status: "running",
        }),
      ]);
      sessionsStore.setVisibleActiveIdForTests("coord");
      await waitFor(() => void headerByName(rendered.root, "Selected Room"));

      for (const name of ["wg-2-dev-team", "Orchestrators", "Selected Room", "Rooms", "Loops"]) {
        expect(collapsed(rendered.root, name)).toBe(false);
        pressOnProxy(rendered.root, name, "Enter");
        await waitFor(() => expect(collapsed(rendered.root, name)).toBe(true));
        pressOnProxy(rendered.root, name, " ");
        await waitFor(() => expect(collapsed(rendered.root, name)).toBe(false));
        click(headerByName(rendered.root, name));
        await waitFor(() => expect(collapsed(rendered.root, name)).toBe(true));
        click(headerByName(rendered.root, name));
        await waitFor(() => expect(collapsed(rendered.root, name)).toBe(false));
      }
    } finally {
      rendered.cleanup();
    }
  });
});
