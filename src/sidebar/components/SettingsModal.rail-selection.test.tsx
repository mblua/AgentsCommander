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
import type { AgentConfig } from "../../shared/types";

// #895 — the configured coding-agent list doubles as the rail picker: clicking
// the first row pins the left/primary rail, clicking any later row targets the
// right/comparison rail. The "Use" button is gone; a trash icon deletes the
// agent without touching the rails.

function agent(id: string, label: string, command: string): AgentConfig {
  return { id, label, command, color: "#334155", envs: [], isolatedHome: false };
}

const AGENTS = [
  agent("codex", "Codex", "codex"),
  agent("claude", "Claude Code", "claude"),
  agent("opencode", "OpenCode", "opencode"),
];

function renderAgents(agents: AgentConfig[] = AGENTS) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  return renderWithFakeTransport(() => <SettingsModal section="agents" onClose={() => {}} />, fake);
}

function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function click(root: HTMLElement, testId: string): void {
  const el = byTestId<HTMLElement>(root, testId);
  if (!el) throw new Error(`missing selector ${testId}`);
  el.click();
}

/** The agent pinned to a rail, or null when the rail is empty. */
function railAgent(root: HTMLElement, railIndex: 0 | 1): string | null {
  return byTestId(root, `settings.profileRail.${railIndex}`)?.getAttribute("data-ac-agent-id") ?? null;
}

async function ready(root: HTMLElement): Promise<void> {
  await waitFor(() => expect(byTestId(root, "settings.agentRow.0.select")).toBeTruthy());
}

describe("SettingsModal coding-agent rail selection (#895)", () => {
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

  it("renders no Use button on any configured agent row", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(r.root.querySelector('[data-ac-testid$=".use"]')).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  it("assigns the left rail when the first row is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      // Pin the left rail away from the first agent via the rail's own selector,
      // so the row-0 click has something to actually change.
      const select = byTestId<HTMLSelectElement>(r.root, "settings.profileRail.0.agentSelect")!;
      select.value = "opencode";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      await waitFor(() => expect(railAgent(r.root, 0)).toBe("opencode"));

      click(r.root, "settings.agentRow.0.select");
      await waitFor(() => expect(railAgent(r.root, 0)).toBe("codex"));
      expect(byTestId(r.root, "settings.agentRow.0")?.getAttribute("data-ac-rail")).toBe("left");
    } finally {
      r.cleanup();
    }
  });

  it("assigns the right rail when a row after the first is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      // Seeded pair: left=codex (agents[0]), right=claude (agents[1]).
      expect(railAgent(r.root, 1)).toBe("claude");

      click(r.root, "settings.agentRow.2.select");
      await waitFor(() => expect(railAgent(r.root, 1)).toBe("opencode"));
      expect(railAgent(r.root, 0)).toBe("codex");
      expect(byTestId(r.root, "settings.agentRow.2")?.getAttribute("data-ac-rail")).toBe("right");
      expect(byTestId(r.root, "settings.agentRow.1")?.getAttribute("data-ac-rail")).toBe("available");
    } finally {
      r.cleanup();
    }
  });

  it("never points both rails at the same agent when the left agent's row is clicked", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      const select = byTestId<HTMLSelectElement>(r.root, "settings.profileRail.0.agentSelect")!;
      select.value = "opencode";
      select.dispatchEvent(new Event("change", { bubbles: true }));
      await waitFor(() => expect(railAgent(r.root, 0)).toBe("opencode"));

      // Row 2 is the left rail now. Clicking it must not hand it to the right
      // rail as well — the pair would collapse onto one agent.
      click(r.root, "settings.agentRow.2.select");
      await waitFor(() => expect(railAgent(r.root, 0)).toBe("opencode"));
      expect(railAgent(r.root, 1)).toBe("claude");
    } finally {
      r.cleanup();
    }
  });

  it("deletes the agent from the trash button without assigning a rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(railAgent(r.root, 1)).toBe("claude");

      // Row 2 (opencode) is 'available'. Its delete must not leak into the head
      // click that would otherwise move it onto the right rail.
      click(r.root, "settings.agentRow.2.remove");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2")).toBeNull());
      expect(railAgent(r.root, 0)).toBe("codex");
      expect(railAgent(r.root, 1)).toBe("claude");
    } finally {
      r.cleanup();
    }
  });

  it("expands the editor from the chevron without assigning a rail", async () => {
    const r = renderAgents();
    try {
      await ready(r.root);
      expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeNull();

      click(r.root, "settings.agentRow.2.toggle");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeTruthy());
      expect(railAgent(r.root, 1)).toBe("claude");

      click(r.root, "settings.agentRow.2.toggle");
      await waitFor(() => expect(byTestId(r.root, "settings.agentRow.2.editor")).toBeNull());
      expect(railAgent(r.root, 1)).toBe("claude");
    } finally {
      r.cleanup();
    }
  });
});
