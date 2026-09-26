// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SidebarApp from "./App";
import { registerShortcuts } from "../shared/shortcuts";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../shared/testing/ui-harness";

vi.mock("../shared/shortcuts", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../shared/shortcuts")>();
  return { ...actual, registerShortcuts: vi.fn(actual.registerShortcuts) };
});

const LATE_LISTENERS = [
  "mousedown",
  "contextmenu",
  "main-sidebar-side-change",
  "focus",
  "visibilitychange",
];

describe("SidebarApp mount disposal", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  // #2628 R1 - an unmount while the last listener of a mount phase is still
  // registering must stop the mount: no later phase may run after onCleanup.
  it("registers nothing after an unmount during the loop_event registration", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("drain_session_warnings", []);
    fake.resolve("get_update_status", null);

    let releaseLoopListen: () => void = () => {};
    const loopListenGate = new Promise<void>((resolve) => {
      releaseLoopListen = resolve;
    });
    let loopListenRequested = false;
    const listen = fake.listen.bind(fake);
    fake.listen = (async (event, callback, options) => {
      if (event === "loop_event") {
        loopListenRequested = true;
        await loopListenGate;
      }
      return listen(event, callback, options);
    }) as FakeTransport["listen"];

    // No unhandled rejection either: vitest fails the run on one.
    const rendered = renderWithFakeTransport(() => <SidebarApp embedded />, fake);
    try {
      await waitFor(() => expect(loopListenRequested).toBe(true));
      // The loop_event listen is parked on the gate; unmount, then release it.
      rendered.cleanup();
      const docAdd = vi.spyOn(document, "addEventListener");
      const winAdd = vi.spyOn(window, "addEventListener");
      vi.mocked(registerShortcuts).mockClear();
      releaseLoopListen();
      await new Promise((resolve) => setTimeout(resolve, 20));

      expect(registerShortcuts).not.toHaveBeenCalled();
      const added = [...docAdd.mock.calls, ...winAdd.mock.calls].map(([type]) => type);
      expect(added.filter((type) => LATE_LISTENERS.includes(type))).toEqual([]);
      expect(fake.calls.some((call) => call.cmd === "get_settings")).toBe(false);
    } finally {
      releaseLoopListen();
    }
  });
});
