// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import type { AgentConfig } from "../../shared/types";

// #2681 - Settings > Coding Agents rows show the weekly quota remaining next
// to the agent name, only with a valid reading, updated live.

function agent(id: string, label: string, command: string): AgentConfig {
  return { id, label, command, color: "#334155", envs: [], isolatedHome: false };
}

function renderAgents() {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents: [agent("codex", "Codex", "codex"), agent("claude", "Claude Code", "claude")] }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  return renderWithFakeTransport(() => <SettingsModal section="agents" onClose={() => {}} />, fake);
}

function row(root: HTMLElement, i: number): HTMLElement {
  const el = root.querySelector<HTMLElement>(`[data-ac-testid="settings.agentRow.${i}"]`);
  if (!el) throw new Error(`missing settings.agentRow.${i}`);
  return el;
}

describe("SettingsModal weekly quota remaining (#2681)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    sessionsStore.resetQuotaReadingsForTests();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("shows_the_label_only_without_a_reading", async () => {
    const r = renderAgents();
    try {
      await waitFor(() => expect(r.root.querySelector('[data-ac-testid="settings.agentRow.1"]')).toBeTruthy());
      const claude = row(r.root, 1);
      expect(claude.querySelector(".agent-quota-name")?.textContent).toBe("Claude Code");
      const head = claude.querySelector<HTMLElement>('[data-ac-testid="settings.agentRow.1.select"]')!;
      expect(head.textContent).toContain("Claude Code");
      expect(claude.textContent).not.toContain("% left");
      expect(claude.querySelector(".agent-quota-remaining")).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  it("shows_remaining_for_the_agent_with_a_reading_and_updates_live", async () => {
    const r = renderAgents();
    try {
      await waitFor(() => expect(r.root.querySelector('[data-ac-testid="settings.agentRow.1"]')).toBeTruthy());

      sessionsStore.setAgentQuota("claude", 28);
      expect(row(r.root, 1).textContent).toContain("72% left");
      expect(row(r.root, 0).querySelector(".agent-quota-remaining")).toBeNull();

      const badge = row(r.root, 1).querySelector(".agent-quota-remaining")!;
      const name = row(r.root, 1).querySelector(".agent-quota-name")!;
      expect(badge.parentElement).toBe(name.parentElement);
      expect(badge.parentElement?.classList.contains("agent-quota-name-line")).toBe(true);

      sessionsStore.setAgentQuota("claude", 40);
      expect(row(r.root, 1).textContent).toContain("60% left");
      sessionsStore.setAgentQuota("claude", null);
      expect(row(r.root, 1).querySelector(".agent-quota-remaining")).toBeNull();
    } finally {
      r.cleanup();
    }
  });
});
