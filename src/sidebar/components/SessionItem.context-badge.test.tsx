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

// #2482 p6 - the weekly-quota fill on the origin agent chip. quotaChipAttrs (p4)
// decides fill vs plain; this pins that SessionItem spreads it onto the chip.
describe("SessionItem agent chip weekly-quota fill (#2482)", () => {
  let cleanupDom: (() => void) | null = null;
  const otherSessionId = "s2";

  function chip(root: ParentNode, id = sessionId): HTMLElement {
    const all = root.querySelectorAll<HTMLElement>(`[data-ac-testid="session.${id}"] .ac-discovery-badge.agent`);
    expect(all.length).toBe(1);
    return all[0];
  }

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    sessionsStore.resetQuotaReadingsForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    sessionsStore.resetQuotaReadingsForTests();
    document.body.replaceChildren();
  });

  it("the_agent_chip_carries_no_quota_class_and_no_inline_style_without_a_reading", async () => {
    const rendered = await renderRow(baseSettings({ agents: [agentConfig()] }));
    try {
      await waitFor(() => expect(chip(rendered.root).textContent).toBe("Claude Code"));
      const el = chip(rendered.root);
      expect(el.className).not.toContain("quota-fill");
      expect(el.getAttribute("style")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("the_agent_chip_sets_the_remaining_custom_property_from_the_reading", async () => {
    sessionsStore.setAgentQuota("claude", 28);
    const rendered = await renderRow(baseSettings({ agents: [agentConfig()] }));
    try {
      await waitFor(() => expect(chip(rendered.root).className).toContain("quota-fill"));
      expect(chip(rendered.root).style.getPropertyValue("--ac-quota-remaining")).toBe("72%");
    } finally {
      rendered.cleanup();
    }
  });

  it("a_null_reading_clears_a_previously_set_fill", async () => {
    sessionsStore.setAgentQuota("claude", 40);
    const rendered = await renderRow(baseSettings({ agents: [agentConfig()] }));
    try {
      await waitFor(() => expect(chip(rendered.root).style.getPropertyValue("--ac-quota-remaining")).toBe("60%"));
      sessionsStore.setAgentQuota("claude", null);
      await waitFor(() => {
        const el = chip(rendered.root);
        expect(el.className).not.toContain("quota-fill");
        expect(el.getAttribute("style")).toBeNull();
      });
    } finally {
      rendered.cleanup();
    }
  });

  it("a_reading_on_an_agent_fills_every_session_chip_of_that_agent", async () => {
    sessionsStore.setAgentQuota("claude", 50);
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ agents: [agentConfig()] }));
    const rendered = renderWithFakeTransport(
      () => (
        <>
          <SessionItem session={session({ id: sessionId, agentId: "claude", agentLabel: "Claude Code" })} isActive={false} />
          <SessionItem session={session({ id: otherSessionId, agentId: "claude", agentLabel: "Claude Code" })} isActive={false} />
        </>
      ),
      fake,
    );
    await settingsStore.load();
    try {
      await waitFor(() => expect(chip(rendered.root, otherSessionId).className).toContain("quota-fill"));
      for (const id of [sessionId, otherSessionId]) {
        const el = chip(rendered.root, id);
        expect(el.className).toContain("quota-fill");
        expect(el.style.getPropertyValue("--ac-quota-remaining")).toBe("50%");
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("a_reading_on_one_agent_does_not_fill_a_session_of_a_different_agent", async () => {
    sessionsStore.setAgentQuota("claude", 50);
    const fake = new FakeTransport();
    fake.resolve(
      "get_settings",
      baseSettings({ agents: [agentConfig(), agentConfig({ id: "codex", label: "Codex", command: "codex" })] }),
    );
    const rendered = renderWithFakeTransport(
      () => (
        <>
          <SessionItem session={session({ id: sessionId, agentId: "claude", agentLabel: "Claude Code" })} isActive={false} />
          <SessionItem session={session({ id: otherSessionId, agentId: "codex", agentLabel: "Codex" })} isActive={false} />
        </>
      ),
      fake,
    );
    await settingsStore.load();
    try {
      await waitFor(() => expect(chip(rendered.root).className).toContain("quota-fill"));
      const el = chip(rendered.root, otherSessionId);
      expect(el.textContent).toBe("Codex");
      expect(el.className).not.toContain("quota-fill");
      expect(el.getAttribute("style")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("the_chip_text_and_the_unfilled_chip_markup_are_unchanged", async () => {
    const rendered = await renderRow(baseSettings({ agents: [agentConfig()] }));
    try {
      await waitFor(() => expect(chip(rendered.root).textContent).toBe("Claude Code"));
      const el = chip(rendered.root);
      expect(el.getAttribute("class")!.split(/\s+/).sort()).toEqual(["ac-discovery-badge", "agent"]);
      expect(el.getAttributeNames().sort()).toEqual(["class"]);
    } finally {
      rendered.cleanup();
    }
  });
});
