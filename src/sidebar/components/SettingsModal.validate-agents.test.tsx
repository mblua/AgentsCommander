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
import type { AgentConfig, CodingAgentEnv, ProfileCellConfig } from "../../shared/types";

// #2611 — validateAgents was split into helpers. These tests pin its messages
// and its order (agents, then profile cells; first error wins) through the
// rendered footer error and the Save path.

function agent(command: string, envs: CodingAgentEnv[] = []): AgentConfig {
  return { id: "codex", label: "Codex", command, color: "#334155", envs, isolatedHome: false };
}

function cell(enabled: boolean, command: string): ProfileCellConfig {
  return { enabled, command, env: {}, notes: "" };
}

function render(agents: AgentConfig[], cells: Record<string, ProfileCellConfig> = {}) {
  const base = baseSettings({ agents });
  const settings = baseSettings({
    agents,
    codingAgentProfiles: {
      ...base.codingAgentProfiles,
      profilesByAgent: { ...base.codingAgentProfiles.profilesByAgent, codex: cells },
    },
  });
  const fake = new FakeTransport();
  fake.resolve("get_settings", settings);
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  fake.resolve("save_settings_draft", null);
  const rendered = renderWithFakeTransport(
    () => <SettingsModal section="agents" onClose={() => {}} />,
    fake,
  );
  return { fake, rendered };
}

function q<T extends Element = HTMLElement>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

async function ready(root: HTMLElement): Promise<void> {
  await waitFor(() => expect(q(root, "settings.agentRow.0.select")).toBeTruthy());
}

function footerError(root: HTMLElement): string {
  return root.querySelector(".modal-save-error")?.textContent ?? "";
}

async function expectFooterError(root: HTMLElement, text: string): Promise<void> {
  await ready(root);
  await waitFor(() => expect(footerError(root)).toBe(text));
}

const CODEX_RESUME_ERROR =
  ": Codex commands must not include resume or --last; AgentsCommander injects codex resume --last automatically";

describe("SettingsModal validateAgents (#2611)", () => {
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

  it("(a) reports a bad env row on an agent", async () => {
    const { rendered } = render([
      agent("codex", [{ key: " ", value: "x", source: "user", enabled: true }]),
    ]);
    try {
      await expectFooterError(rendered.root, 'Agent "Codex": Environment variable keys cannot be empty.');
      expect(q<HTMLButtonElement>(rendered.root, "settings.save")!.disabled).toBe(true);
    } finally {
      rendered.cleanup();
    }
  });

  it("(b) reports an unclosed quote in an agent command", async () => {
    const { rendered } = render([agent('codex "--model')]);
    try {
      await expectFooterError(rendered.root, 'Agent "Codex": Unclosed " quote');
    } finally {
      rendered.cleanup();
    }
  });

  it("(c) reports codex resume in an enabled profile cell", async () => {
    const { rendered } = render([agent("codex")], { B: cell(true, "codex resume --last") });
    try {
      await expectFooterError(rendered.root, `Profile codex:B${CODEX_RESUME_ERROR}`);
    } finally {
      rendered.cleanup();
    }
  });

  it("(d) ignores an invalid disabled profile cell and saves", async () => {
    const { fake, rendered } = render([agent("codex")], {
      B: cell(false, "codex resume --last"),
      C: cell(false, '"unclosed'),
    });
    try {
      await expectFooterError(rendered.root, "");
      const save = q<HTMLButtonElement>(rendered.root, "settings.save")!;
      expect(save.disabled).toBe(false);
      save.click();
      await waitFor(() => expect(fake.callsFor("save_settings_draft")).toHaveLength(1));
    } finally {
      rendered.cleanup();
    }
  });

  it("(e) shows the agent error first when an agent and a cell are both invalid", async () => {
    const { rendered } = render([agent("codex resume")], { B: cell(true, '"unclosed') });
    try {
      await expectFooterError(rendered.root, `Agent "Codex"${CODEX_RESUME_ERROR}`);
    } finally {
      rendered.cleanup();
    }
  });
});
