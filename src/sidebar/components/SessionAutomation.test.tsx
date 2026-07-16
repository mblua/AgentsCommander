// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createStore } from "solid-js/store";
import SessionItem from "./SessionItem";
import RootAgentBanner from "./RootAgentBanner";
import iconUrl from "../../../src-tauri/icons/64x64.png";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  contextMenu,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
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

      const detach = rendered.root.querySelector('[data-ac-testid="rootAgent.detachToggle"]');
      expect(detach?.getAttribute("data-ac-state")).toBe("attached");
      click(detach!);
      await waitFor(() =>
        expect(fake.lastCall("detach_terminal")?.args).toEqual({ sessionId: "root-1" })
      );

      const destroy = rendered.root.querySelector('[data-ac-testid="rootAgent.destroy"]');
      expect(destroy).not.toBeNull();
      click(destroy!);
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
    const fake = new FakeTransport();
    fake.resolve("destroy_session", undefined);
    fake.resolve("restart_session", session({ ...root, status: "running" }));
    fake.resolve("switch_session", undefined);
    const rendered = renderWithFakeTransport(() => <RootAgentBanner />, fake);
    try {
      const banner = rendered.root.querySelector('[data-ac-testid="rootAgent.banner"]');
      expect(banner?.getAttribute("data-ac-state")).toBe("dormant");
      expect(rendered.root.querySelector(".session-item-mic")).toBeNull();
      expect(rendered.root.querySelector(".session-item-detach")).toBeNull();
      expect(rendered.root.querySelector(".session-item-telegram")).toBeNull();
      const destroy = rendered.root.querySelector('[data-ac-testid="rootAgent.destroy"]');
      expect(destroy).not.toBeNull();
      click(destroy!);
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
      const close = rendered.root.querySelector('[data-ac-testid="rootAgent.destroy"]');
      click(close!);
      click(close!);
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
