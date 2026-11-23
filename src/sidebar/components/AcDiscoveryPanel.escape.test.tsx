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

  const replicaMenu = () => document.body.querySelector<HTMLElement>(".session-context-menu");
  const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
  const countWindow = (type: string) => {
    const seen: Event[] = [];
    const fn = (e: Event) => seen.push(e);
    window.addEventListener(type, fn);
    return { seen, stop: () => window.removeEventListener(type, fn) };
  };
  const openReplicaMenu = async () => {
    await waitFor(() => expect(rendered.root.querySelector(".replica-item")).not.toBeNull());
    contextMenu(rendered.root.querySelector(".replica-item")!);
    await waitFor(() => expect(replicaMenu()).not.toBeNull());
    await tick();
  };
  const clickEvt = (el: Element) =>
    el.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));

  it("replica menu propagation: interior click stays open with zero window clicks", async () => {
    await openReplicaMenu();
    const win = countWindow("click");
    clickEvt(replicaMenu()!);
    win.stop();
    expect(win.seen).toHaveLength(0);
    expect(replicaMenu()).not.toBeNull();
  });

  it("replica menu propagation: child menu button acts", async () => {
    await openReplicaMenu();
    const option = Array.from(replicaMenu()!.querySelectorAll<HTMLElement>("button"))
      .find((el) => el.textContent?.trim() === "Context Files")!;
    clickEvt(option);
    await waitFor(() => expect(overlay()).not.toBeNull());
    expect(replicaMenu()).toBeNull();
  });

  it("replica menu propagation: non-Escape key keeps it open; Escape dismisses", async () => {
    await openReplicaMenu();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "a", cancelable: true }));
    expect(replicaMenu()).not.toBeNull();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    await waitFor(() => expect(replicaMenu()).toBeNull());
  });

  it("replica menu propagation: outside click dismisses", async () => {
    await openReplicaMenu();
    clickEvt(document.body);
    await waitFor(() => expect(replicaMenu()).toBeNull());
  });

  it("Context Files: backdrop click closes", async () => {
    await openPanel();
    clickEvt(overlay()!);
    await waitFor(() => expect(overlay()).toBeNull());
  });

  it("Context Files: document Escape closes", async () => {
    await openPanel();
    press(document.body, "Escape");
    await waitFor(() => expect(overlay()).toBeNull());
  });

  it("Context Files: native Close button closes", async () => {
    await openPanel();
    const close = document.body.querySelector<HTMLButtonElement>(".ctx-files-close")!;
    expect(close.tagName).toBe("BUTTON");
    clickEvt(close);
    await waitFor(() => expect(overlay()).toBeNull());
  });

  it("Context Files: nested interior click keeps it open, zero window clicks", async () => {
    await openPanel();
    const win = countWindow("click");
    const nested = document.body.querySelector<HTMLElement>(".ctx-files-header")!;
    clickEvt(nested);
    clickEvt(panel());
    win.stop();
    expect(win.seen).toHaveLength(0);
    expect(overlay()).not.toBeNull();
    clickEvt(document.body.querySelector(".ctx-files-close")!);
    await waitFor(() => expect(overlay()).toBeNull());
  });
});
