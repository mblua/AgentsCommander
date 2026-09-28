// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
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
import { ROW_KEY_PROXY_CLASS } from "./RowKeyProxy";

// #2659 - the agent and replica rows open the Coding Agent picker by keyboard
// through their row key proxy; mouse click and right-click are unchanged.
const agentRowSel = '[data-ac-testid="acDiscovery.agent.proj-dev"]';
const replicaRowSel = '[data-ac-testid="acDiscovery.replica.wg-1-team.dev"]';
const filterSel = '[data-ac-testid="agentPicker.agentFilter"]';

const press = (key: string) => (row: HTMLElement) => {
  const proxy = row.querySelector<HTMLElement>(`:scope > .${ROW_KEY_PROXY_CLASS}`);
  expect(row.firstElementChild).toBe(proxy);
  proxy!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
};

describe("AcDiscoveryPanel row keyboard access (#2659)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    const fake = new FakeTransport();
    fake.resolve("discover_ac_agents", discovery({
      agents: [{ name: "proj/dev", path: "C:\\Project\\.ac\\_agent_dev", roleExists: true }],
      workgroups: [{
        name: "wg-1-team",
        path: "C:\\Project\\.ac\\wg-1-team",
        task: null,
        taskTitle: null,
        agents: [{ name: "dev", path: "C:\\Project\\.ac\\wg-1-team\\__agent_dev", repoPaths: [], isCoordinator: false }],
      }],
    }));
    fake.resolve("get_settings", baseSettings({
      agents: [{ id: "codex", label: "Codex", command: "codex", color: "#10b981", envs: [], isolatedHome: false }],
    }));
    fake.resolve("get_replica_context_files", []);
    rendered = renderWithFakeTransport(() => <AcDiscoveryPanel />, fake);
  });

  afterEach(() => {
    rendered.cleanup();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  const row = async (sel: string) => {
    await waitFor(() => expect(rendered.root.querySelector(sel)).not.toBeNull());
    return rendered.root.querySelector<HTMLElement>(sel)!;
  };

  for (const [label, sel] of [["agent", agentRowSel], ["replica", replicaRowSel]] as const) {
    for (const [name, act] of [["Enter", press("Enter")], ["Space", press(" ")], ["click", click]] as const) {
      it(`opens the picker once from the ${label} row with ${name}`, async () => {
        const r = await row(sel);
        expect(document.querySelector(filterSel)).toBeNull();
        act(r);
        await waitFor(() => expect(document.querySelector(filterSel)).not.toBeNull());
        expect(document.querySelectorAll(filterSel)).toHaveLength(1);
      });
    }
  }

  it("right-click on the replica row still opens its context menu", async () => {
    contextMenu(await row(replicaRowSel));
    await waitFor(() => expect(document.body.querySelector(".session-context-option")).not.toBeNull());
    expect(document.querySelector(filterSel)).toBeNull();
  });
});
