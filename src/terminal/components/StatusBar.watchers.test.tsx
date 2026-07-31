// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// #1171 test 78 needs both sides of the `isTauri` gate in one file, so the module is
// mocked behind a mutable flag rather than probed from the environment.
let tauriEnvironment = true;
vi.mock("../../shared/platform", () => ({
  get isTauri() {
    return tauriEnvironment;
  },
  get isBrowser() {
    return !tauriEnvironment;
  },
  isWindows: false,
}));

import StatusBar from "./StatusBar";
import { terminalStore } from "../stores/terminal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
} from "../../shared/testing/ui-harness";

const WATCHERS_BTN = '[data-ac-testid="statusBar.watchers"]';

describe("the StatusBar watcher-activity button (#1171)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    tauriEnvironment = true;
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    terminalStore.setActiveSessionForTests(null);
    document.body.replaceChildren();
  });

  function renderStatusBar() {
    const fake = new FakeTransport();
    return renderWithFakeTransport(() => <StatusBar />, fake);
  }

  it("renders whenever there is an active session, independent of the TASK panel", () => {
    terminalStore.setActiveSessionForTests("session-1");
    const rendered = renderStatusBar();
    try {
      const button = rendered.root.querySelector(WATCHERS_BTN);
      expect(button).toBeTruthy();
      // Inside `.status-bar-actions`, NOT `.workgroup-task-actions`. That placement is the
      // whole reason it survives on a root-agent session, where the TASK bar is not
      // rendered at all.
      expect(button?.closest(".status-bar-actions")).toBeTruthy();
      expect(button?.closest(".workgroup-task-actions")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("does not render at all without an active session", () => {
    terminalStore.setActiveSessionForTests(null);
    const rendered = renderStatusBar();
    try {
      expect(rendered.root.querySelector(WATCHERS_BTN)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  // The web client renders this same StatusBar, but has no watchers window and no arm for
  // `open_watchers_window`, so the button would return a raw error. Absent beats broken.
  it("does not render in the web client", () => {
    tauriEnvironment = false;
    terminalStore.setActiveSessionForTests("session-1");
    const rendered = renderStatusBar();
    try {
      expect(rendered.root.querySelector(WATCHERS_BTN)).toBeNull();
      // The neighbouring buttons are still there, so this is the gate and not a dead bar.
      expect(rendered.root.querySelector(".status-bar-btn-clear")).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("asks the backend to open the window scoped to the active session", () => {
    terminalStore.setActiveSessionForTests("session-42");
    const fake = new FakeTransport();
    fake.resolve("open_watchers_window", undefined);
    const rendered = renderWithFakeTransport(() => <StatusBar />, fake);
    try {
      rendered.root.querySelector<HTMLButtonElement>(WATCHERS_BTN)!.click();
      expect(fake.lastCall("open_watchers_window")?.args).toEqual({
        sessionId: "session-42",
      });
    } finally {
      rendered.cleanup();
    }
  });
});
