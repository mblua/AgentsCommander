// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SessionItem from "./SessionItem";
import RootAgentBanner from "./RootAgentBanner";
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
          skipAutoResume: null,
        })
      );
    } finally {
      rendered.cleanup();
    }
  });
});
