// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AcDiscoveryPanel from "./AcDiscoveryPanel";
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

const replicaPath = "C:\\Project\\.ac\\wg-1-team\\__agent_dev";

const overlay = () => document.body.querySelector<HTMLElement>(".ctx-files-overlay");
const panel = () => document.body.querySelector<HTMLElement>(".ctx-files-panel")!;
const input = () => document.body.querySelector<HTMLInputElement>(".ctx-files-input")!;
const press = (el: Element, key: string) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));

describe("AcDiscoveryPanel context files overlay keyboard (#2660)", () => {
  let cleanupDom: (() => void) | null = null;
  let fake: FakeTransport;
  let rendered: ReturnType<typeof renderWithFakeTransport>;

  const openPanel = async () => {
    await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
    contextMenu(rendered.root.querySelector(".replica-item")!);
    const option = Array.from(document.body.querySelectorAll<HTMLElement>(".session-context-option")).find(
      (el) => el.textContent?.trim() === "Context Files"
    )!;
    click(option);
    await waitFor(() => expect(document.body.querySelector(".ctx-files-input")).not.toBeNull());
  };

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    fake = new FakeTransport();
    fake.resolve("discover_ac_agents", discovery({
      workgroups: [{
        name: "wg-1-team",
        path: "C:\\Project\\.ac\\wg-1-team",
        task: null,
        taskTitle: null,
        agents: [{ name: "dev", path: replicaPath, repoPaths: [], isCoordinator: false }],
      }],
    }));
    fake.resolve("get_replica_context_files", []);
    fake.resolve("set_replica_context_files", null);
    rendered = renderWithFakeTransport(() => <AcDiscoveryPanel />, fake);
  });

  afterEach(() => {
    rendered.cleanup();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("closes on Escape from the panel or the input, and on backdrop click", async () => {
    for (const close of [() => press(panel(), "Escape"), () => press(input(), "Escape"), () => click(overlay()!)]) {
      await openPanel();
      close();
      await waitFor(() => expect(overlay()).toBeNull());
    }
  });

  it("removes the Escape listener once the panel is closed", async () => {
    await openPanel();
    const removeSpy = vi.spyOn(document, "removeEventListener");
    click(overlay()!);
    await waitFor(() => expect(overlay()).toBeNull());
    expect(removeSpy).toHaveBeenCalledWith("keydown", expect.any(Function));
    removeSpy.mockRestore();
    const reads = fake.callsFor("get_replica_context_files").length;
    press(document.body, "Escape");
    expect(overlay()).toBeNull();
    expect(fake.callsFor("get_replica_context_files")).toHaveLength(reads);
  });

  it("keeps Enter in the path input adding the file", async () => {
    await openPanel();
    input().value = "Role.md";
    input().dispatchEvent(new InputEvent("input", { bubbles: true }));
    press(input(), "Enter");
    await waitFor(() => expect(fake.callsFor("set_replica_context_files")).toHaveLength(1));
    expect(overlay()).not.toBeNull();
  });
});
