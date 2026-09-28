// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { ROW_KEY_PROXY_CLASS } from "./RowKeyProxy";
import { automationIdPart } from "./replica-repo-badges";
import { loopSummaryFixture as loop } from "./loop-summary-fixture";

// #2658 - Agents/Teams/team headers, the loop row and the offline Agent Matrix row
// run their click action by keyboard through their row key proxy.
const root = "C:\\Project";
const agent = "AgentsCommander_ac/__agent_dev-webpage-ui";
const projectId = automationIdPart(root);
const loopId = automationIdPart("loop-standup");

const q = (selector: string) => document.body.querySelector<HTMLElement>(selector);
const find = (selector: string, match: (el: HTMLElement) => boolean): HTMLElement => {
  const found = Array.from(document.querySelectorAll<HTMLElement>(selector)).find(match);
  if (!found) throw new Error(`Not found: ${selector}`);
  return found;
};
const byText = (selector: string, text: string) => find(selector, (el) => !!el.textContent?.includes(text));
const header = (name: string) =>
  find(".ac-wg-header--collapsible", (h) => h.querySelector(".ac-wg-name")?.textContent?.trim() === name);
const teamHeader = () => byText(".ac-team-header", "frontend-team");
const collapsed = (row: HTMLElement) =>
  row.querySelector(".ac-discovery-chevron")!.classList.contains("collapsed");
const loopRow = () => q(`[data-ac-testid="loop.row.${projectId}.${loopId}"]`)!;
const agentRow = () => byText(".replica-item", "dev-webpage-ui");
const loopEditor = () => q('[data-ac-testid="loop.edit.name"]');
const notice = () => q('[data-ac-testid="agentMatrixNotice.modal"]');

/** Enter/Space reach the row through its proxy, which must be the row's first child. */
const press = (key: string) => (row: HTMLElement) => {
  const proxy = row.querySelector<HTMLElement>(`:scope > .${ROW_KEY_PROXY_CLASS}`);
  expect(row.firstElementChild).toBe(proxy);
  proxy!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
};

describe("ProjectPanel Phase B row keyboard access", () => {
  // The returned teardown runs after each test.
  beforeEach(() => {
    const restoreDom = installBrowserDomStubs();
    resetUiStoresForTests();
    return () => {
      restoreDom();
      resetUiStoresForTests();
      document.body.replaceChildren();
    };
  });

  const mount = async () => {
    const fake = new FakeTransport();
    const replies: Record<string, unknown> = {
      new_project: { path: root, registered: true, created: false },
      discover_project: discovery({
        agents: [{ name: agent, path: `${root}\\.ac\\_agent_dev-webpage-ui`, roleExists: true }],
        teams: [{ name: "frontend-team", agents: [agent], coordinator: agent }],
        loops: [loop()],
      }),
      list_unresolved_loop_targets: [],
    };
    for (const [command, reply] of Object.entries(replies)) fake.resolve(command, reply);
    const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
    await projectStore.createAndLoad(root);
    await waitFor(() => void loopRow().tagName);
    return rendered;
  };

  it("toggles Agents, Teams and the team header with Enter, Space and mouse click", async () => {
    const rendered = await mount();
    try {
      const rows: Array<() => HTMLElement> = [() => header("Agents"), () => header("Teams"), teamHeader];
      for (const row of rows) {
        // Four toggles leave each header expanded, so the team header stays rendered.
        for (const act of [press("Enter"), press(" "), click, click]) {
          const before = collapsed(row());
          act(row());
          await waitFor(() => expect(collapsed(row())).toBe(!before));
        }
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("opens the loop editor with Enter, Space and click, and right-click still opens its menu", async () => {
    const rendered = await mount();
    const closeEditor = async () => {
      document.querySelector(".modal-overlay")!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await waitFor(() => expect(loopEditor()).toBeNull());
    };
    try {
      for (const act of [press("Enter"), press(" "), click]) {
        expect(loopEditor()).toBeNull();
        act(loopRow());
        await waitFor(() => expect(loopEditor()).not.toBeNull());
        expect(document.querySelectorAll('[data-ac-testid="loop.edit.name"]')).toHaveLength(1);
        await closeEditor();
      }
      contextMenu(loopRow());
      await waitFor(() => expect(q(`[data-ac-testid="loop.action.toggle.${projectId}.${loopId}"]`)).not.toBeNull());
    } finally {
      rendered.cleanup();
    }
  });

  it("opens the Agent Matrix notice from the offline agent row with Enter, Space and click", async () => {
    const rendered = await mount();
    try {
      for (const act of [press("Enter"), press(" "), click]) {
        expect(notice()).toBeNull();
        act(agentRow());
        await waitFor(() => expect(notice()).not.toBeNull());
        expect(document.querySelectorAll('[data-ac-testid="agentMatrixNotice.modal"]')).toHaveLength(1);
        document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        await waitFor(() => expect(notice()).toBeNull());
      }
    } finally {
      rendered.cleanup();
    }
  });
});
