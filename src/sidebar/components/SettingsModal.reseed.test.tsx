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
import { toastStore } from "../../shared/stores/toasts";
import type { AgentConfig, CodingAgentDefinition } from "../../shared/types";

const RESEED_CMD = "reseed_coding_agent_default";
const LIST_CMD = "list_reseedable_agent_commands";

function def(key: string, label: string, command: string, dest?: string): CodingAgentDefinition {
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
    ...(dest ? { configSeed: { enabled: true, dest } } : {}),
  };
}

// Mirrors the Phase-2 default catalog: configSeed.dest present exactly for the
// three re-seedable built-ins.
const CATALOG: CodingAgentDefinition[] = [
  def("claude", "Claude Code", "claude", ".claude"),
  def("codex", "Codex", "codex", ".codex"),
  def("hermes", "Hermes", "hermes"),
  def("cursor", "Cursor CLI", "agent"),
  def("pi", "Pi", "pi"),
  def("opencode", "OpenCode", "opencode", ".opencode"),
];

const RESEEDABLE = ["claude", "codex", "opencode"];

function agent(command: string, id = "a1", label = "Agent"): AgentConfig {
  return { id, label, command, color: "#334155", envs: [], isolatedHome: false };
}

interface Opts {
  catalog?: CodingAgentDefinition[];
  reseedable?: string[];
  reseedableReject?: boolean;
  reseedResolve?: { dest: string; backupPath: string };
  reseedReject?: string;
}

function renderAgents(agents: AgentConfig[], opts: Opts = {}) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", opts.catalog ?? CATALOG);
  if (opts.reseedableReject) fake.reject(LIST_CMD, "boom");
  else fake.resolve(LIST_CMD, opts.reseedable ?? RESEEDABLE);
  if (opts.reseedReject !== undefined) fake.reject(RESEED_CMD, opts.reseedReject);
  else fake.resolve(RESEED_CMD, opts.reseedResolve ?? { dest: ".claude", backupPath: "" });
  return renderWithFakeTransport(() => <SettingsModal section="agents" onClose={() => {}} />, fake);
}

function click(el: Element | null): void {
  (el as HTMLButtonElement | null)?.click();
}

async function expandRow(root: HTMLElement, i: number): Promise<void> {
  await waitFor(() => expect(root.querySelector(`[data-ac-testid="settings.agentRow.${i}.toggle"]`)).toBeTruthy());
  click(root.querySelector(`[data-ac-testid="settings.agentRow.${i}.toggle"]`));
  await waitFor(() => expect(root.querySelector(`[data-ac-testid="settings.agentRow.${i}.editor"]`)).toBeTruthy());
}

function reseedBtn(root: HTMLElement, key: string): HTMLButtonElement | null {
  return root.querySelector<HTMLButtonElement>(`[data-ac-testid="settings.agent.reseedDefault.${key}"]`);
}

describe("SettingsModal re-seed default configuration (#769 Phase 2)", () => {
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

  it("shows the button (correct testid) for a re-seedable built-in when expanded", async () => {
    const r = renderAgents([agent("claude", "c", "Claude")]);
    try {
      await expandRow(r.root, 0);
      await waitFor(() => expect(reseedBtn(r.root, "claude")).toBeTruthy());
    } finally {
      r.cleanup();
    }
  });

  it("hides the button for Cursor CLI (agent), Hermes, Pi and custom commands", async () => {
    const r = renderAgents([
      agent("agent", "cur", "Cursor CLI"),
      agent("hermes", "her", "Hermes"),
      agent("pi", "pi", "Pi"),
      agent("mytool --flag", "cus", "Custom"),
    ]);
    try {
      for (let i = 0; i < 4; i++) {
        await expandRow(r.root, i);
        // No re-seed button of any key should appear for these rows.
        expect(
          r.root.querySelector('[data-ac-testid^="settings.agent.reseedDefault."]'),
        ).toBeNull();
      }
    } finally {
      r.cleanup();
    }
  });

  it("gates by EXACT basename, not .startsWith (`pip` gets no button, `pi` does)", async () => {
    const r = renderAgents([agent("pip", "pip", "Pip"), agent("pi", "pi", "Pi")], {
      catalog: [def("pi", "Pi", "pi", ".pi")],
      reseedable: ["pi"],
    });
    try {
      await expandRow(r.root, 0); // pip
      expect(r.root.querySelector('[data-ac-testid^="settings.agent.reseedDefault."]')).toBeNull();
      await expandRow(r.root, 1); // pi
      await waitFor(() => expect(reseedBtn(r.root, "pi")).toBeTruthy());
    } finally {
      r.cleanup();
    }
  });

  it("confirm → calls reseed with the command and toasts success including the backup path", async () => {
    const backupPath = "C:/cfg/coding-agents/_seed/.claude.bak-20260703";
    const r = renderAgents([agent("claude", "c", "Claude")], {
      reseedResolve: { dest: ".claude", backupPath },
    });
    try {
      await expandRow(r.root, 0);
      await waitFor(() => expect(reseedBtn(r.root, "claude")).toBeTruthy());
      click(reseedBtn(r.root, "claude"));
      await waitFor(() =>
        expect(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]')).toBeTruthy(),
      );
      click(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]'));

      await waitFor(() =>
        expect(
          toastStore.items.some((t) => t.kind === "success" && t.message.includes(backupPath)),
        ).toBe(true),
      );
      const call = r.fake.callsFor(RESEED_CMD)[0];
      expect(call?.args).toEqual({ command: "claude" });
    } finally {
      r.cleanup();
    }
  });

  it("omits the backup clause when backupPath is empty", async () => {
    const r = renderAgents([agent("claude", "c", "Claude")], {
      reseedResolve: { dest: ".claude", backupPath: "" },
    });
    try {
      await expandRow(r.root, 0);
      await waitFor(() => expect(reseedBtn(r.root, "claude")).toBeTruthy());
      click(reseedBtn(r.root, "claude"));
      await waitFor(() =>
        expect(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]')).toBeTruthy(),
      );
      click(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]'));

      await waitFor(() =>
        expect(toastStore.items.some((t) => t.kind === "success")).toBe(true),
      );
      const success = toastStore.items.find((t) => t.kind === "success")!;
      expect(success.message).toBe("Default configuration restored");
      expect(success.message).not.toContain("backup at");
    } finally {
      r.cleanup();
    }
  });

  it("shows a non-destructive error toast when re-seed fails", async () => {
    const r = renderAgents([agent("claude", "c", "Claude")], { reseedReject: "not reseedable" });
    try {
      await expandRow(r.root, 0);
      await waitFor(() => expect(reseedBtn(r.root, "claude")).toBeTruthy());
      click(reseedBtn(r.root, "claude"));
      await waitFor(() =>
        expect(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]')).toBeTruthy(),
      );
      click(r.root.querySelector('[data-ac-testid="settings.reseedConfirm.confirm"]'));

      await waitFor(() => expect(toastStore.items.some((t) => t.kind === "error")).toBe(true));
    } finally {
      r.cleanup();
    }
  });

  it("shows NO buttons when the reseedable-set fetch fails (failure-safe)", async () => {
    const r = renderAgents([agent("claude", "c", "Claude")], { reseedableReject: true });
    try {
      await expandRow(r.root, 0);
      // Give ensureLoaded a beat to settle its failed reseedable fetch.
      await waitFor(() =>
        expect(r.root.querySelector(`[data-ac-testid="settings.agentRow.0.editor"]`)).toBeTruthy(),
      );
      expect(reseedBtn(r.root, "claude")).toBeNull();
    } finally {
      r.cleanup();
    }
  });
});
