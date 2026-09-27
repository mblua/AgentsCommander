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
import { ROW_KEY_PROXY_CLASS } from "./RowKeyProxy";
import { loopSummaryFixture as loop } from "./loop-summary-fixture";

// #2657 - the 5 collapsible ProjectPanel headers toggle by keyboard through their row key proxy.
const root = "C:\\Project";
const room = `${root}\\.ac\\wg-2-dev-team`;
const agent = "AgentsCommander_ac/__agent_dev-webpage-ui";

const header = (name: string): HTMLElement => {
  const found = Array.from(document.querySelectorAll<HTMLElement>(".ac-wg-header--collapsible"))
    .find((h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === name);
  if (!found) throw new Error(`Header not found: ${name}`);
  return found;
};
const collapsed = (name: string) =>
  header(name).querySelector(".ac-discovery-chevron")!.classList.contains("collapsed");

const press = (key: string) => (row: HTMLElement) => {
  const proxy = row.firstElementChild!;
  expect(proxy.classList.contains(ROW_KEY_PROXY_CLASS)).toBe(true);
  proxy.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
};

describe("ProjectPanel header keyboard access", () => {
  let cleanupDom: (() => void) | null = null;
  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });
  afterEach(() => {
    cleanupDom?.();
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("toggles each collapsible header with Enter, Space and mouse click", async () => {
    const fake = new FakeTransport();
    fake.resolve("new_project", { path: root, registered: true, created: false });
    fake.resolve("discover_project", discovery({
      agents: [{ name: agent, path: `${root}\\.ac\\_agent_dev-webpage-ui`, roleExists: true }],
      teams: [{ name: "frontend-team", agents: [agent], coordinator: agent }],
      workgroups: [{
        name: "wg-2-dev-team", path: room, task: null, taskTitle: "Task",
        agents: [{ name: "dev-webpage-ui", path: `${room}\\__agent_dev-webpage-ui`, repoPaths: [], isCoordinator: true }],
      }],
      loops: [loop()],
    }));
    // An active orchestrator session makes the Selected Room header render.
    sessionsStore.setSessions([session({
      id: "coord", name: "wg-2-dev-team/dev-webpage-ui",
      workingDirectory: `${room}\\__agent_dev-webpage-ui`, status: "running",
    })]);
    sessionsStore.setVisibleActiveIdForTests("coord");

    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    const toggles = async (name: string, act: (row: HTMLElement) => void, expected: boolean) => {
      act(header(name));
      await waitFor(() => expect(collapsed(name)).toBe(expected));
    };
    try {
      await projectStore.createAndLoad(root);
      await waitFor(() => void header("Selected Room"));
      for (const name of ["wg-2-dev-team", "Orchestrators", "Selected Room", "Rooms", "Loops"]) {
        expect(collapsed(name)).toBe(false);
        await toggles(name, press("Enter"), true);
        await toggles(name, press(" "), false);
        await toggles(name, click, true);
        await toggles(name, click, false);
      }
    } finally {
      rendered.cleanup();
    }
  });
});
