// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

// #2659 - both AC Agents rows (matrix and replica) open the launch picker from
// their key proxy exactly like a click does.
describe("AcDiscoveryPanel row keyboard access (#2659)", () => {
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

  const picker = () => document.body.querySelector(".agent-modal");

  for (const [kind, title] of [["matrix", "C:\\P\\.ac\\_agent_dev"], ["replica", "C:\\P\\.ac\\wg-1-t\\__agent_dev"]]) {
    for (const act of ["Enter", " ", "click"]) {
      it(`opens the picker from the ${kind} row on ${JSON.stringify(act)}`, async () => {
        const fake = new FakeTransport();
        fake.resolve("get_settings", baseSettings());
        fake.resolve("discover_ac_agents", discovery({
          agents: [{ name: "dev", path: "C:\\P\\.ac\\_agent_dev", roleExists: true }],
          workgroups: [{
            name: "wg-1-t",
            path: "C:\\P\\.ac\\wg-1-t",
            task: null,
            taskTitle: null,
            agents: [{ name: "dev", path: "C:\\P\\.ac\\wg-1-t\\__agent_dev", repoPaths: [], isCoordinator: false }],
          }],
        }));
        const rendered = renderWithFakeTransport(() => <AcDiscoveryPanel />, fake);
        try {
          const row = () =>
            Array.from(rendered.root.querySelectorAll<HTMLElement>(".replica-item")).find((el) => el.title === title);
          await waitFor(() => expect(row()).not.toBeNull());
          const proxy = row()!.firstElementChild!;
          expect(proxy.classList.contains("ac-row-key-proxy")).toBe(true);
          expect(picker()).toBeNull();
          if (act === "click") row()!.click();
          else proxy.dispatchEvent(new KeyboardEvent("keydown", { key: act, bubbles: true }));
          await waitFor(() => expect(picker()).not.toBeNull());
        } finally {
          rendered.cleanup();
        }
      });
    }
  }
});
