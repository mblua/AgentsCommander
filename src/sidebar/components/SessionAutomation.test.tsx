// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createStore } from "solid-js/store";
import SessionItem from "./SessionItem";
import RootAgentBanner from "./RootAgentBanner";
import iconUrl from "../../../src-tauri/icons/64x64.png";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  bridge,
  click,
  contextMenu,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { voiceRecorder } from "../../shared/voice-recorder";

describe("session workflow automation hooks", () => {
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

  it("exposes regular session row, restart, destroy, and detach/attach targets", async () => {
    const fake = new FakeTransport();
    fake.resolve("detach_terminal", "terminal-session1");
    fake.resolve("attach_terminal", undefined);
    fake.resolve("destroy_session", undefined);
    fake.resolve("restart_session", session({ id: "session-1" }));

    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({ id: "session-1", name: "General" })}
          isActive={true}
        />
      ),
      fake
    );
    try {
      const row = rendered.root.querySelector('[data-ac-testid="session.session-1"]');
      expect(row?.getAttribute("data-ac-state")).toBe("active");

      const detach = rendered.root.querySelector('[data-ac-testid="session.session-1.detachToggle"]');
      expect(detach?.getAttribute("data-ac-state")).toBe("attached");
      click(detach!);
      await waitFor(() =>
        expect(fake.lastCall("detach_terminal")?.args).toEqual({ sessionId: "session-1" })
      );

      const destroy = rendered.root.querySelector('[data-ac-testid="session.session-1.destroy"]');
      expect(destroy).not.toBeNull();
      click(destroy!);
      await waitFor(() =>
        expect(fake.lastCall("destroy_session")?.args).toEqual({ id: "session-1" })
      );

      contextMenu(row!);
      await waitFor(() => {
        expect(document.querySelector('[data-ac-testid="session.session-1.menu"]')).not.toBeNull();
        expect(
          document.querySelector('[data-ac-testid="session.session-1.restart"]')
        ).not.toBeNull();
        expect(
          document.querySelector('[data-ac-testid="session.session-1.menu.detachToggle"]')
        ).not.toBeNull();
      });

      click(document.querySelector('[data-ac-testid="session.session-1.restart"]')!);
      await waitFor(() =>
        expect(fake.lastCall("restart_session")?.args).toEqual({
          id: "session-1",
          agentId: null,
          requestedProfile: null,
          skipAutoResume: null,
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("exposes root-agent workflow targets", async () => {
    const root = session({
      id: "root-1",
      name: "Agent's Commander",
      isRootAgent: true,
      status: "running",
    });
    sessionsStore.setSessions([root]);

    const fake = new FakeTransport();
    fake.resolve("detach_terminal", "terminal-root1");
    fake.resolve("destroy_session", undefined);
    fake.resolve("restart_session", root);
    fake.resolve("switch_session", undefined);

    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const banner = rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]');
      expect(banner?.getAttribute("data-ac-state")).toBe("live");
      const avatar = rendered.root.querySelector<HTMLImageElement>(".root-agent-avatar-img");
      expect(avatar?.getAttribute("src")).toBe(iconUrl);
      expect(avatar?.getAttribute("alt")).toBe("");
      expect(rendered.root.querySelector(".root-agent-avatar svg")).toBeNull();

      // #1896 - a live, quiet banner carries no row buttons; every action is
      // menu-only. The banner itself is a div with role="button", so the count
      // is over descendants. Quiet: not recording, no auto-execute countdown,
      // profileOutdated unset (ProfileOutdatedBadge renders a <button>).
      expect(banner!.querySelectorAll("button")).toHaveLength(0);
      for (const cls of [
        "session-item-mic",
        "session-item-explorer",
        "session-item-detach",
        "session-item-telegram",
        "session-item-close",
        "session-item-bot-menu",
      ]) {
        expect(banner!.querySelector(`.${cls}`)).toBeNull();
      }
      expect(rendered.root.querySelector('[data-ac-testid="rootAgent.detachToggle"]')).toBeNull();
      expect(rendered.root.querySelector('[data-ac-testid="rootAgent.destroy"]')).toBeNull();

      contextMenu(banner!);
      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="rootAgent.menu.detachToggle"]')).not.toBeNull()
      );
      const detach = document.querySelector('[data-ac-testid="rootAgent.menu.detachToggle"]');
      expect(detach?.getAttribute("data-ac-state")).toBe("attached");
      click(detach!);
      expect(document.querySelector('[data-ac-testid="rootAgent.menu"]')).toBeNull();
      await waitFor(() =>
        expect(fake.lastCall("detach_terminal")?.args).toEqual({ sessionId: "root-1" })
      );

      contextMenu(banner!);
      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="rootAgent.close"]')).not.toBeNull()
      );
      click(document.querySelector('[data-ac-testid="rootAgent.close"]')!);
      await waitFor(() =>
        expect(fake.lastCall("destroy_session")?.args).toEqual({ id: "root-1" })
      );

      contextMenu(banner!);
      await waitFor(() => {
        expect(document.querySelector('[data-ac-testid="rootAgent.menu"]')).not.toBeNull();
        expect(document.querySelector('[data-ac-testid="rootAgent.restart"]')).not.toBeNull();
        expect(document.querySelector('[data-ac-testid="rootAgent.menu.detachToggle"]')).not.toBeNull();
      });

      click(document.querySelector('[data-ac-testid="rootAgent.restart"]')!);
      await waitFor(() =>
        expect(fake.lastCall("restart_session")?.args).toEqual({
          id: "root-1",
          agentId: null,
          requestedProfile: null,
          skipAutoResume: null,
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("removes every PTY-dependent regular-row control and gates stale handlers when it becomes dormant", async () => {
    const [row, setRow] = createStore(session({ id: "session-dormant", status: "running" }));
    const fake = new FakeTransport();
    fake.resolve("detach_terminal", "terminal-sessiondormant");
    fake.resolve("telegram_list_bridges", []);
    fake.resolve("destroy_session", undefined);
    const toggle = vi.spyOn(voiceRecorder, "toggle").mockImplementation(() => undefined);
    const rendered = renderWithFakeTransport(
      () => <SessionItem session={row} isActive={true} />,
      fake,
    );
    try {
      const staleMic = rendered.root.querySelector(".session-item-mic");
      const staleDetach = rendered.root.querySelector(".session-item-detach");
      const staleTelegram = rendered.root.querySelector(".session-item-telegram");
      expect(staleMic).not.toBeNull();
      expect(staleDetach).not.toBeNull();
      expect(staleTelegram).not.toBeNull();

      setRow("status", { exited: 17 });
      expect(rendered.root.querySelector(".session-item-mic")).toBeNull();
      expect(rendered.root.querySelector(".session-item-detach")).toBeNull();
      expect(rendered.root.querySelector(".session-item-telegram")).toBeNull();
      expect(
        rendered.root.querySelector('[data-ac-testid="session.session-dormant.destroy"]'),
      ).not.toBeNull();

      click(staleMic!);
      click(staleDetach!);
      click(staleTelegram!);
      await Promise.resolve();
      expect(toggle).not.toHaveBeenCalled();
      expect(fake.callsFor("detach_terminal")).toHaveLength(0);
      expect(fake.callsFor("telegram_attach")).toHaveLength(0);
      expect(fake.callsFor("telegram_detach")).toHaveLength(0);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps dormant Root wake and close while hiding PTY controls and preserving its exit code", async () => {
    const root = session({
      id: "root-dormant",
      name: "Agent's Commander",
      isRootAgent: true,
      status: { exited: 137 },
    });
    sessionsStore.setSessions([root]);
    // #1896 - a bridge still recorded for a dormant root must show no indicator:
    // the span is live-gated exactly like the replica row's.
    bridgesStore.setBridges([bridge({ sessionId: "root-dormant" })]);
    const fake = new FakeTransport();
    fake.resolve("destroy_session", undefined);
    fake.resolve("restart_session", session({ ...root, status: "running" }));
    fake.resolve("switch_session", undefined);
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const banner = rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]');
      expect(banner?.getAttribute("data-ac-state")).toBe("dormant");
      expect(banner!.querySelectorAll("button")).toHaveLength(0);
      expect(rendered.root.querySelector(".session-item-mic")).toBeNull();
      expect(rendered.root.querySelector(".session-item-detach")).toBeNull();
      expect(rendered.root.querySelector(".session-item-telegram")).toBeNull();
      expect(rendered.root.querySelector(".session-item-explorer")).toBeNull();
      expect(rendered.root.querySelector(".session-item-close")).toBeNull();
      expect(rendered.root.querySelector(".session-item-bridge-icon")).toBeNull();
      expect(rendered.root.querySelector('[data-ac-testid="rootAgent.destroy"]')).toBeNull();

      contextMenu(banner!);
      await waitFor(() =>
        expect(document.querySelector('[data-ac-testid="rootAgent.close"]')).not.toBeNull()
      );
      expect(document.querySelector('[data-ac-testid="rootAgent.menu.detachToggle"]')).toBeNull();
      click(document.querySelector('[data-ac-testid="rootAgent.close"]')!);
      await waitFor(() => expect(fake.callsFor("destroy_session")).toHaveLength(1));
      expect(sessionsStore.sessions[0]?.status).toEqual({ exited: 137 });
      expect(fake.callsFor("switch_session")).toHaveLength(0);

      click(banner!);
      await waitFor(() => expect(fake.callsFor("restart_session")).toHaveLength(1));
      expect(fake.lastCall("restart_session")?.args).toEqual({
        id: "root-dormant",
        agentId: null,
        requestedProfile: null,
        skipAutoResume: false,
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("catches a dormant Root close rejection once and releases its busy gate", async () => {
    const root = session({
      id: "root-dormant",
      name: "Agent's Commander",
      isRootAgent: true,
      status: { exited: 23 },
    });
    sessionsStore.setSessions([root]);
    let rejectDestroy = (_reason: unknown): void => undefined;
    const destroy = new Promise<never>((_resolve, reject) => {
      rejectDestroy = reject;
    });
    const fake = new FakeTransport();
    fake.onInvoke("destroy_session", () => destroy);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const banner = rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]');
      const closeItem = () => document.querySelector('[data-ac-testid="rootAgent.close"]');
      // #1896 - close is menu-only. An item dismisses the menu before it
      // selects, so the second attempt needs its own open; handleContextMenu has
      // no busy gate, menuClose does.
      contextMenu(banner!);
      await waitFor(() => expect(closeItem()).not.toBeNull());
      click(closeItem()!);
      expect(closeItem()).toBeNull();
      contextMenu(banner!);
      await waitFor(() => expect(closeItem()).not.toBeNull());
      click(closeItem()!);
      expect(fake.callsFor("destroy_session")).toHaveLength(1);
      rejectDestroy("destroy-failed");
      await waitFor(() => expect(error).toHaveBeenCalledOnce());
      expect(fake.callsFor("destroy_session")).toHaveLength(1);
      expect(
        rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]')?.getAttribute("aria-disabled"),
      ).toBe("false");
      expect(sessionsStore.sessions[0]?.status).toEqual({ exited: 23 });
    } finally {
      rendered.cleanup();
    }
  });

  it("#592: surfaces the Root Agent drift reload badge and relaunches on click", async () => {
    // The Root Agent can drift too (its loaded profile cell vs current config), and
    // its hash is persisted (a80a1a7). The banner must show the same reload badge as
    // SessionItem / replica rows, wired to the root restart path.
    const root = session({
      id: "root-1",
      name: "Agent's Commander",
      isRootAgent: true,
      status: "running",
      profileOutdated: true,
    });
    sessionsStore.setSessions([root]);

    const fake = new FakeTransport();
    fake.resolve("restart_session", root);
    fake.resolve("switch_session", undefined);

    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const badge = rendered.root.querySelector(".profile-outdated-badge");
      expect(badge).not.toBeNull();

      click(badge!);
      await waitFor(() =>
        expect(fake.lastCall("restart_session")?.args).toEqual({
          id: "root-1",
          agentId: null,
          requestedProfile: null,
          skipAutoResume: null,
        })
      );
    } finally {
      rendered.cleanup();
    }
  });

  it("#592: hides the Root Agent drift badge when the profile is current", async () => {
    const root = session({
      id: "root-1",
      name: "Agent's Commander",
      isRootAgent: true,
      status: "running",
      profileOutdated: false,
    });
    sessionsStore.setSessions([root]);

    const fake = new FakeTransport();
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      expect(rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]')).not.toBeNull();
      expect(rendered.root.querySelector(".profile-outdated-badge")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});

describe("Telegram indicator after #1730", () => {
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

  it("#1896: shows the button-less Telegram indicator on the root banner while a bridge is attached", async () => {
    sessionsStore.setSessions([session({ id: "root-1", isRootAgent: true, status: "running" })]);
    // A named colour: jsdom rewrites hex in style.color to rgb(), the replica
    // test (ProjectPanel.context-menu.test.tsx) uses "red" for the same reason.
    bridgesStore.setBridges([bridge({ sessionId: "root-1", color: "red" })]);

    const fake = new FakeTransport();
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const icon = rendered.root.querySelector<HTMLElement>(".root-agent-banner .session-item-bridge-icon");
      expect(icon).not.toBeNull();
      expect(icon?.tagName).toBe("SPAN");
      expect(icon?.style.color).toBe("red");
      expect(icon?.getAttribute("title")).toBe("Telegram: Ops Bot");
      expect(icon?.querySelector("svg")).not.toBeNull();
      expect(rendered.root.querySelector(".session-item-telegram")).toBeNull();
      expect(rendered.root.querySelector(".session-item-bridge-dot")).toBeNull();
      expect(rendered.root.querySelector(".root-agent-banner button")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("#1896: renders neither indicator nor Telegram button on the root banner with no bridge", async () => {
    sessionsStore.setSessions([session({ id: "root-1", isRootAgent: true, status: "running" })]);
    bridgesStore.setBridges([]);

    const fake = new FakeTransport();
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      expect(rendered.root.querySelector(".session-item-bridge-icon")).toBeNull();
      expect(rendered.root.querySelector(".session-item-telegram")).toBeNull();
      expect(rendered.root.querySelector(".session-item-bridge-dot")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("leaves a session row button titled Attach Telegram with no bridge", async () => {
    const fake = new FakeTransport();
    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({ id: "session-1", status: "running" })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      const button = rendered.root.querySelector(".session-item-telegram");
      expect(button).not.toBeNull();
      expect(button?.getAttribute("title")).toBe("Attach Telegram");
      expect(rendered.root.querySelector(".session-item-bridge-dot")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
