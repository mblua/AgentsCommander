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
const HOLD_ICON = '[data-ac-testid="statusBar.typingHoldIcon"]';

// #2379 - the counter never renders a `#`, and the padlock state is asserted
// through the monochrome SVG (the color-emoji glyph could not be painted).
function holdIcon(root: HTMLElement): SVGElement {
  return root.querySelector<SVGElement>(HOLD_ICON)!;
}
function holdCount(root: HTMLElement): HTMLSpanElement | null {
  return root.querySelector<HTMLSpanElement>(".status-bar-hold-count");
}

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
      await waitFor(() =>
        expect(button.getAttribute("aria-label")).toBe(
          "Hold message delivery to this session (0 held)",
        ),
      );
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("open");
      expect(holdIcon(rendered.root).getAttribute("fill")).toBe("currentColor");
      expect(button.textContent).not.toContain("#");
      expect(holdCount(rendered.root)).toBeNull();
      // Same action group as the watcher and clear-input controls.
      expect(button.closest(".status-bar-actions")).toBeTruthy();
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
      await waitFor(() => expect(holdCount(rendered.root)?.textContent).toBe("2"));
      const button = holdButton(rendered.root);
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("closed");
      expect(button.textContent).not.toContain("#");
      expect(button.getAttribute("aria-label")).toBe(
        "Release held messages and resume delivery (2 held)",
      );
      expect(button.getAttribute("aria-pressed")).toBe("true");
    } finally {
      rendered.cleanup();
    }
  });

  it("shows a closed padlock with a 0 count when nothing is held", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: true, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdCount(rendered.root)?.textContent).toBe("0"));
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("closed");
      expect(holdButton(rendered.root).textContent).not.toContain("#");
    } finally {
      rendered.cleanup();
    }
  });

  it("does not claim a held count before the first snapshot arrives", async () => {
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    let resolveGet!: (value: unknown) => void;
    fake.onInvoke("get_typing_hold", () =>
      new Promise((resolve) => {
        resolveGet = resolve;
      }),
    );
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root)).toBeTruthy());
      expect(holdButton(rendered.root).getAttribute("title")).toBe(
        "Hold message delivery to this session",
      );
      resolveGet({ closed: false, heldCount: 0 });
      await waitFor(() =>
        expect(holdButton(rendered.root).getAttribute("aria-label")).toBe(
          "Hold message delivery to this session (0 held)",
        ),
      );
      expect(holdButton(rendered.root).getAttribute("aria-label")).not.toContain("#");
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
      expect(holdCount(rendered.root)).toBeNull();
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("open");
    } finally {
      rendered.cleanup();
    }
  });

  it("stops polling the old session after a switch", async () => {
    vi.useFakeTimers();
    terminalStore.setActiveSessionForTests("session-A");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    const rendered = renderStatusBar(fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(500);
      const callsForA = () =>
        fake.callsFor("get_typing_hold").filter((call) => call.args.sessionId === "session-A")
          .length;
      expect(callsForA()).toBe(2);

      terminalStore.setActiveSessionForTests("session-B");
      await vi.advanceTimersByTimeAsync(0);
      const aCallsAtSwitch = callsForA();

      // Two more seconds of ticks: A's interval must be gone, B's must run.
      await vi.advanceTimersByTimeAsync(2000);
      expect(callsForA()).toBe(aCallsAtSwitch);
      expect(
        fake.callsFor("get_typing_hold").some((call) => call.args.sessionId === "session-B"),
      ).toBe(true);
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
      expect(holdCount(rendered.root)?.textContent).toBe("1");
    } finally {
      rendered.cleanup();
    }
  });

  it("drops a poll that resolves after a toggle, so it cannot repaint pre-toggle state", async () => {
    vi.useFakeTimers();
    terminalStore.setActiveSessionForTests("session-1");
    const fake = new FakeTransport();
    let pollCount = 0;
    let resolvePendingPoll!: (value: unknown) => void;
    fake.onInvoke("get_typing_hold", () => {
      pollCount += 1;
      if (pollCount === 1) return Promise.resolve({ closed: false, heldCount: 0 });
      return new Promise((resolve) => {
        resolvePendingPoll = resolve;
      });
    });
    let resolveToggle!: (value: unknown) => void;
    fake.onInvoke("toggle_typing_hold", () =>
      new Promise((resolve) => {
        resolveToggle = resolve;
      }),
    );
    const rendered = renderStatusBar(fake);
    try {
      await vi.advanceTimersByTimeAsync(0);
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("open");
      // A poll is in flight when the click happens; it still answers with the
      // pre-toggle snapshot and must not land after the toggle result.
      await vi.advanceTimersByTimeAsync(500);
      expect(pollCount).toBe(2);

      click(holdButton(rendered.root));
      resolveToggle({ closed: true, heldCount: 2 });
      await vi.advanceTimersByTimeAsync(0);
      expect(holdCount(rendered.root)?.textContent).toBe("2");

      resolvePendingPoll({ closed: false, heldCount: 0 });
      await vi.advanceTimersByTimeAsync(0);
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("closed");
      expect(holdCount(rendered.root)?.textContent).toBe("2");
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the new tab's padlock usable while another tab's toggle is pending", async () => {
    terminalStore.setActiveSessionForTests("session-A");
    const fake = new FakeTransport();
    fake.resolve("get_typing_hold", { closed: false, heldCount: 0 });
    let resolveToggleA!: (value: unknown) => void;
    fake.onInvoke("toggle_typing_hold", (args) =>
      args.sessionId === "session-A"
        ? new Promise((resolve) => {
            resolveToggleA = resolve;
          })
        : Promise.resolve({ closed: true, heldCount: 1 }),
    );
    const rendered = renderStatusBar(fake);
    try {
      await waitFor(() => expect(holdButton(rendered.root)).toBeTruthy());
      click(holdButton(rendered.root));
      await waitFor(() => expect(fake.callsFor("toggle_typing_hold")).toHaveLength(1));
      expect(holdButton(rendered.root).disabled).toBe(true);

      terminalStore.setActiveSessionForTests("session-B");
      await waitFor(() =>
        expect(
          fake.callsFor("get_typing_hold").some((call) => call.args.sessionId === "session-B"),
        ).toBe(true),
      );
      // B's padlock is not the pending one, so it stays enabled and clickable.
      expect(holdButton(rendered.root).disabled).toBe(false);
      click(holdButton(rendered.root));
      await waitFor(() => expect(fake.callsFor("toggle_typing_hold")).toHaveLength(2));
      expect(fake.lastCall("toggle_typing_hold")!.args).toEqual({ sessionId: "session-B" });
      resolveToggleA({ closed: true, heldCount: 5 });
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
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("open");
      snapshot = { closed: true, heldCount: 4 };
      click(holdButton(rendered.root));
      await vi.advanceTimersByTimeAsync(0);
      // The refetch happened without waiting for the next 500 ms poll.
      expect(getCalls).toBe(2);
      expect(holdCount(rendered.root)?.textContent).toBe("4");
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
      expect(holdIcon(rendered.root).getAttribute("data-state")).toBe("open");
      // ...and the next tick retries.
      await vi.advanceTimersByTimeAsync(500);
      expect(attempts).toBe(2);
      expect(holdCount(rendered.root)?.textContent).toBe("3");

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
