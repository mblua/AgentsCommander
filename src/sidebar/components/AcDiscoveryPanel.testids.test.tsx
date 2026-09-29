// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
import {
  click,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { acDiscoveryTransport, agentRowSel, filterSel, replicaRowSel } from "./ac-discovery-fixture";

// #2528: automation testids on the AcDiscoveryPanel rows that open the Coding Agent picker.

describe("AcDiscoveryPanel automation testids (#2528)", () => {
  let cleanupDom: (() => void) | null = null;
  let rendered: ReturnType<typeof renderWithFakeTransport>;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    const fake = acDiscoveryTransport();
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
