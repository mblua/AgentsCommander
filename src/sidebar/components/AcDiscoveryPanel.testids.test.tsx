// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

// #2528: automation testids on the AcDiscoveryPanel rows that open the Coding Agent picker.
const agentRowSel = '[data-ac-testid="acDiscovery.agent.proj-dev"]';
const replicaRowSel = '[data-ac-testid="acDiscovery.replica.wg-1-team.dev"]';
const filterSel = '[data-ac-testid="agentPicker.agentFilter"]';

describe("AcDiscoveryPanel automation testids (#2528)", () => {
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
    // The picker filter renders only with at least one coding agent.
    fake.resolve("get_settings", baseSettings({
      agents: [{ id: "codex", label: "Codex", command: "codex", color: "#10b981", envs: [], isolatedHome: false }],
    }));
    rendered = renderWithFakeTransport(() => <AcDiscoveryPanel />, fake);
  });

  afterEach(() => {
    rendered.cleanup();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  const rows = async () => {
    await waitFor(() => expect(rendered.root.querySelector(replicaRowSel)).not.toBeNull());
    return {
      agentRows: rendered.root.querySelectorAll<HTMLElement>(agentRowSel),
      replicaRows: rendered.root.querySelectorAll<HTMLElement>(replicaRowSel),
    };
  };

  it("tags the agent row with a unique button testid", async () => {
    const { agentRows } = await rows();
    expect(agentRows).toHaveLength(1);
    expect(agentRows[0].getAttribute("data-ac-role")).toBe("button");
  });

  it("tags the replica row with a unique button testid", async () => {
    const { replicaRows } = await rows();
    expect(replicaRows).toHaveLength(1);
    expect(replicaRows[0].getAttribute("data-ac-role")).toBe("button");
  });

  for (const [label, sel] of [["agent", agentRowSel], ["replica", replicaRowSel]] as const) {
    it(`opens the Coding Agent picker from the tagged ${label} row`, async () => {
      await rows();
      expect(document.querySelector(filterSel)).toBeNull();
      click(rendered.root.querySelector<HTMLElement>(sel)!);
      await waitFor(() => expect(document.querySelector(filterSel)).not.toBeNull());
    });
  }

  it("keeps row class, text and no ARIA role", async () => {
    const { agentRows, replicaRows } = await rows();
    for (const [row, text] of [[agentRows[0], "proj/dev"], [replicaRows[0], "dev"]] as const) {
      expect(row.getAttribute("class")).toBe("replica-item");
      expect(row.textContent).toContain(text);
      expect(row.hasAttribute("role")).toBe(false);
    }
  });
});
