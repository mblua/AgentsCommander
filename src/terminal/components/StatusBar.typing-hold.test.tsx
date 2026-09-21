// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Both sides of the `isTauri` gate in one file, like the watcher-activity suite:
// the module is mocked behind a mutable flag rather than probed from the
// environment.
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
  click,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

const HOLD_BTN = '[data-ac-testid="statusBar.typingHold"]';
const OPEN = "\u{1F513}";
const CLOSED = "\u{1F512}";

// #2337 — the padlock is a mirror of the backend's per-session typing hold.
// These tests pin the states it must show, the session it must target, the
// 500 ms poll (retry + cleanup), and the web-client absence.

describe("the StatusBar typing-hold padlock (#2337)", () => {
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
    vi.useRealTimers();
  });

  function renderStatusBar(fake: FakeTransport) {
    return renderWithFakeTransport(() => <StatusBar />, fake);
  }

  function holdButton(root: HTMLElement): HTMLButtonElement {
    return root.querySelector<HTMLButtonElement>(HOLD_BTN)!;
  }

  it("renders the open padlock when the backend reports no hold", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root)).toBeTruthy());
      const button = holdButton(rendered.root);
      expect(button.textContent).toContain(OPEN);
      expect(button.textContent).not.toContain("#");
      // Same action group as the watcher and clear-input controls.
      expect(button.closest(".status-bar-actions")).toBeTruthy();
      // Accessible name explains the action; no closed count is claimed.
      expect(button.getAttribute("aria-label")).toContain("Hold message delivery");
      expect(button.getAttribute("title")).toBe(button.getAttribute("aria-label"));
    } finally {
      rendered.cleanup();
    }
  });

  it("renders the closed padlock with the exact held count", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: true, heldCount: 2 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root).textContent).toContain("#2"));
      const button = holdButton(rendered.root);
      expect(button.textContent).toContain(CLOSED);
      expect(button.getAttribute("aria-label")).toBe(
        "Release held messages and resume delivery (#2 held)",
      );
      expect(button.getAttribute("aria-pressed")).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });

  it("shows a closed padlock with #0 when nothing is held", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: true, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root).textContent).toContain("#0"));
      expect(holdButton(rendered.root).textContent).toContain(CLOSED);
    } finally {
      rendered.cleanup();
    }
  });

  it("toggles the session that is active at click, never a previous tab", async () => {
    terminalStore.setActiveSessionForTests("session-A");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    fake.resolve("toggle_typing_hold", { closed: true, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root)).toBeTruthy());
      click(holdButton(rendered.root));
      await waitFor(() => expect(fake.lastCall("toggle_typing_hold")).toBeTruthy());
      expect(fake.lastCall("toggle_typing_hold")!.args).toEqual({ sessionId: "session-A" });

      terminalStore.setActiveSessionForTests("session-B");
      await waitFor(() =>
        expect(
          fake.callsFor("get_typing_hold").some((call) => call.args.sessionId === "session-B"),
        ).toBe(true),
      );
      click(holdButton(rendered.root));
      await waitFor(() => expect(fake.callsFor("toggle_typing_hold")).toHaveLength(2));
      expect(fake.lastCall("toggle_typing_hold")!.args).toEqual({ sessionId: "session-B" });
    } finally {
      rendered.cleanup();
    }
  });

  it("ignores a late snapshot for a session that is no longer active", async () => {
    terminalStore.setActiveSessionForTests("session-A");
    const fake = new FakeTransport();
    let resolveA!: (value: unknown) => void;
    const pendingA = new Promise((resolve) => {
      resolveA = resolve;
    });
    fake.onInvoke("get_typing_hold", (args) =>
      args.sessionId === "session-A"
        ? pendingA
        : Promise.resolve({ closed: false, heldCount: 0 }),
    );
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(fake.callsFor("get_typing_hold").length).toBeGreaterThan(0));
      terminalStore.setActiveSessionForTests("session-B");
      await waitFor(() =>
        expect(
          fake.callsFor("get_typing_hold").some((call) => call.args.sessionId === "session-B"),
        ).toBe(true),
      );
      resolveA({ closed: true, heldCount: 5 });
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(holdButton(rendered.root).textContent).not.toContain("#5");
    } finally {
      rendered.cleanup();
    }
  });

  it("disables the button while a toggle is pending so one click flips once", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    let resolveToggle!: (value: unknown) => void;
    const pendingToggle = new Promise((resolve) => {
      resolveToggle = resolve;
    });
    fake.onInvoke("toggle_typing_hold", () => pendingToggle);
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root)).toBeTruthy());
      click(holdButton(rendered.root));
      expect(holdButton(rendered.root).disabled).toBe(true);
      click(holdButton(rendered.root));
      expect(fake.callsFor("toggle_typing_hold")).toHaveLength(1);
      resolveToggle({ closed: true, heldCount: 1 });
      await waitFor(() => expect(holdButton(rendered.root).disabled).toBe(false));
      expect(holdButton(rendered.root).textContent).toContain("#1");
    } finally {
      rendered.cleanup();
    }
  });

  it("refetches the authoritative snapshot right after a failed toggle", async () => {
    vi.useFakeTimers();
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    let getCalls = 0;
    let snapshot = { closed: false, heldCount: 0 };
    fake.onInvoke("get_typing_hold", () => {
      getCalls += 1;
      return Promise.resolve(snapshot);
    });
    fake.reject("toggle_typing_hold", "toggle failed");
    const rendered = renderStatusBar(fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(holdButton(rendered.root).textContent).toContain(OPEN);
      snapshot = { closed: true, heldCount: 4 };
      click(holdButton(rendered.root));
      await vi.advanceTimersByTimeAsync(0);
      // The refetch happened without waiting for the next 500 ms poll.
      expect(getCalls).toBe(2);
      expect(holdButton(rendered.root).textContent).toContain("#4");
    } finally {
      rendered.cleanup();
    }
  });

  it("retries a failed poll and stops polling on unmount", async () => {
    vi.useFakeTimers();
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    let attempts = 0;
    fake.onInvoke("get_typing_hold", () => {
      attempts += 1;
      if (attempts === 1) return Promise.reject(new Error("poll failed"));
      return Promise.resolve({ closed: true, heldCount: 3 });
    });
    const rendered = renderStatusBar(fake);
    let unmounted = false;
    try {
      await vi.advanceTimersByTimeAsync(0);
      // A failed poll keeps the last known state (initial open)...
      expect(holdButton(rendered.root).textContent).toContain(OPEN);
      // ...and the next tick retries.
      await vi.advanceTimersByTimeAsync(500);
      expect(attempts).toBe(2);
      expect(holdButton(rendered.root).textContent).toContain("#3");

      rendered.cleanup();
      unmounted = true;
      const callsAfterUnmount = fake.callsFor("get_typing_hold").length;
      await vi.advanceTimersByTimeAsync(2000);
      expect(fake.callsFor("get_typing_hold")).toHaveLength(callsAfterUnmount);
    } finally {
      if (!unmounted) rendered.cleanup();
    }
  });

  it("hides the padlock in the web client, where the commands have no arm", async () => {
    tauriEnvironment = false;
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() =>
        expect(rendered.root.querySelector(".status-bar-btn-clear")).toBeTruthy(),
      );
      expect(rendered.root.querySelector(HOLD_BTN)).toBeNull();
      // No unusable button and no wasted poll against an unexposed command.
      expect(fake.callsFor("get_typing_hold")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });
});
