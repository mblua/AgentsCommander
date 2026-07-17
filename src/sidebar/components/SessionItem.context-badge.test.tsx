// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SessionItem from "./SessionItem";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "../stores/sessions";
import type { AgentConfig, AppSettings, Session } from "../../shared/types";

// #1033 - the CTX badge on the sidebar's own session rows. The listener, the
// hydration and the projection are covered against the real App in
// App.context-badge.workflow.test.tsx; this pins THIS surface's gate and wiring,
// which is the half that cannot be shared and is therefore the half that drifts.

const sessionId = "s1";
const CLAUDE_PATTERN = String.raw`^ {2}Context [░█]+ (\d{1,3})%`;

function agentConfig(overrides: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id: "claude",
    label: "Claude Code",
    command: "claude",
    color: "#d97757",
    envs: [],
    isolatedHome: false,
    ...overrides,
  };
}

function badge(root: ParentNode): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[data-ac-testid="session.${sessionId}.contextBadge"]`);
}

async function renderRow(settings: AppSettings, sessionProps: Partial<Session> = {}) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", settings);
  const rendered = renderWithFakeTransport(
    () => (
      <SessionItem
        session={session({ id: sessionId, agentId: "claude", agentLabel: "Claude Code", ...sessionProps })}
        isActive={false}
      />
    ),
    fake,
  );
  await settingsStore.load();
  return rendered;
}

describe("SessionItem CTX badge (#1033)", () => {
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

  it("renders no badge at all when the agent has no pattern", async () => {
    const rendered = await renderRow(baseSettings({ agents: [agentConfig()] }));
    try {
      // Not N/A, not an empty chip: absent entirely.
      expect(badge(rendered.root)).toBeNull();
      expect(rendered.root.textContent).not.toContain("CTX");
    } finally {
      rendered.cleanup();
    }
  });

  it("renders N/A when a pattern is set but no reading has arrived", async () => {
    const rendered = await renderRow(
      baseSettings({ agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })] }),
    );
    try {
      await waitFor(() => expect(badge(rendered.root)?.textContent).toBe("CTX N/A"));
    } finally {
      rendered.cleanup();
    }
  });

  it("paints the reading the store holds, including a real zero", async () => {
    sessionsStore.setSessionContext(sessionId, 0);
    const rendered = await renderRow(
      baseSettings({ agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })] }),
    );
    try {
      await waitFor(() => {
        expect(badge(rendered.root)?.textContent).toBe("CTX 0%");
        expect(badge(rendered.root)?.getAttribute("role")).toBe("meter");
      });

      sessionsStore.setSessionContext(sessionId, 42);
      await waitFor(() => expect(badge(rendered.root)?.textContent).toBe("CTX 42%"));
    } finally {
      rendered.cleanup();
    }
  });

  it("appears with no reload as soon as a pattern is saved", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ agents: [agentConfig()] }));
    const rendered = renderWithFakeTransport(
      () => (
        <SessionItem
          session={session({ id: sessionId, agentId: "claude", agentLabel: "Claude Code" })}
          isActive={false}
        />
      ),
      fake,
    );
    try {
      await settingsStore.load();
      expect(badge(rendered.root)).toBeNull();

      // What SettingsModal's save does: re-resolve settings into the signal.
      fake.resolve(
        "get_settings",
        baseSettings({ agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })] }),
      );
      await settingsStore.load();

      await waitFor(() => expect(badge(rendered.root)?.textContent).toBe("CTX N/A"));
    } finally {
      rendered.cleanup();
    }
  });

  // Pins the gate at the only place #1033 could break #1031's key-by-id rule: two
  // agents share a command, only one configures a pattern.
  it("keys the gate by agent id and never by command", async () => {
    const rendered = await renderRow(
      baseSettings({
        agents: [
          agentConfig({ id: "claude", contextRegex: CLAUDE_PATTERN }),
          agentConfig({ id: "claude-2", label: "Claude Two" }),
        ],
      }),
      { agentId: "claude-2", agentLabel: "Claude Two" },
    );
    try {
      expect(badge(rendered.root)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows no badge for a plain shell", async () => {
    const rendered = await renderRow(
      baseSettings({ agents: [agentConfig({ contextRegex: CLAUDE_PATTERN })] }),
      { agentId: null, agentLabel: null },
    );
    try {
      expect(badge(rendered.root)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
