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
import { FALLBACK_CODING_AGENTS } from "../../shared/agent-presets";
import type { AgentConfig, CodingAgentDefinition } from "../../shared/types";

const CATALOG_CMD = "get_coding_agent_catalog";

function def(key: string, label: string, command: string): CodingAgentDefinition {
  return {
    key,
    label,
    description: `Coding Agent ${label}`,
    color: "#334155",
    command,
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
  };
}

function existingAgent(command: string): AgentConfig {
  return {
    id: "agent_existing",
    label: `Existing ${command}`,
    command,
    color: "#334155",
    envs: [],
    isolatedHome: false,
  };
}

function presetBtn(root: HTMLElement, key: string): HTMLButtonElement | null {
  return root.querySelector<HTMLButtonElement>(`[data-ac-testid="settings.agentPreset.${key}"]`);
}

describe("SettingsModal coding-agent quick-add row (#769)", () => {
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

  it("renders one quick-add button per fetched definition, in order, and disables ones already added", async () => {
    const fake = new FakeTransport();
    // An agent whose command is `claude` already exists → the Claude quick-add
    // must be disabled by hasAgentByCommand; codex/pi stay available.
    fake.resolve("get_settings", baseSettings({ agents: [existingAgent("claude")] }));
    fake.resolve("get_web_server_status", false);
    fake.resolve(CATALOG_CMD, [
      def("claude", "Claude Code", "claude"),
      def("codex", "Codex", "codex"),
      def("pi", "Pi", "pi"),
    ]);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(presetBtn(rendered.root, "codex")).toBeTruthy());

      // Exactly the three fetched keys, in file order (custom stays separate).
      const keys = Array.from(
        rendered.root.querySelectorAll<HTMLElement>('[data-ac-testid^="settings.agentPreset."]'),
      ).map((el) => el.getAttribute("data-ac-testid"));
      expect(keys).toEqual([
        "settings.agentPreset.claude",
        "settings.agentPreset.codex",
        "settings.agentPreset.pi",
      ]);

      expect(presetBtn(rendered.root, "claude")!.disabled).toBe(true);
      expect(presetBtn(rendered.root, "codex")!.disabled).toBe(false);
      expect(presetBtn(rendered.root, "pi")!.disabled).toBe(false);
      // The hardcoded Custom Agent button is still present, last.
      expect(
        rendered.root.querySelector('[data-ac-testid="settings.agent.addCustom"]'),
      ).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("falls back to the built-in list (never blank) when the catalog fetch fails", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.reject(CATALOG_CMD, "config-dir failure");

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      // All six built-ins from the synchronous fallback render despite the reject.
      await waitFor(() => expect(presetBtn(rendered.root, "opencode")).toBeTruthy());
      for (const d of FALLBACK_CODING_AGENTS) {
        expect(presetBtn(rendered.root, d.key)).toBeTruthy();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the Auto-update error note when the overview is rejected while the presets still fall back (#1551)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.reject(CATALOG_CMD, "config-dir failure");
    fake.reject("get_agent_update_overview", "overview failure");

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      const errorNote = () =>
        rendered.root.querySelector<HTMLElement>('[data-ac-testid="settings.autoUpdate.error"]');
      await waitFor(() => expect(errorNote()).toBeTruthy());
      expect(errorNote()!.textContent).toContain("Auto-update status unavailable: ");
      expect(errorNote()!.textContent).toContain("overview failure");
      // never an empty table, never the loading note
      expect(rendered.root.querySelector('[data-ac-testid="settings.autoUpdate.list"]')).toBeNull();
      expect(rendered.root.querySelector('[data-ac-testid="settings.autoUpdate.loading"]')).toBeNull();
      // the two surfaces are independent: the preset row still falls back to the built-ins
      await waitFor(() => expect(presetBtn(rendered.root, "opencode")).toBeTruthy());
      for (const d of FALLBACK_CODING_AGENTS) {
        expect(presetBtn(rendered.root, d.key)).toBeTruthy();
      }
      expect(fake.listensFor("agent_install_state_changed")).toHaveLength(1);
      expect(fake.listensFor("agent_updates_finished")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });
});
