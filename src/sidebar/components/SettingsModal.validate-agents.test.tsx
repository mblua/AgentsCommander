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

// #3021 - base/profile labels and canonical keys on save after a mixed-key reload.
describe("SettingsModal command field labels and writes (#3021)", () => {
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

  const CARD = "settings.profileCard.0.A";

  function renderWire() {
    const base = baseSettings();
    const { command: _command, ...agentRest } = agent("unused");
    const wire = {
      ...base,
      agents: [{ ...agentRest, baseCommand: "", command: 'codex --profile "x y"' }],
      codingAgentProfiles: {
        ...base.codingAgentProfiles,
        profilesByAgent: { codex: {
          A: { enabled: true, env: {}, notes: "", additionalArguments: " ", command: "--legacy" },
          B: { enabled: false, env: {}, notes: "", additionalArguments: "--new-off", command: "--old-off" },
        } },
      },
    };
    const fake = new FakeTransport();
    fake.resolve("get_settings", wire);
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

  it("labels the base and profile fields and keeps the legacy fallback visible on reload", async () => {
    const { rendered } = renderWire();
    try {
      await ready(rendered.root);
      await waitFor(() => expect(q(rendered.root, `${CARD}.command`)).toBeTruthy());
      const baseInput = q<HTMLInputElement>(rendered.root, "settings.agentRow.0.command")!;
      expect(baseInput.closest("label")?.querySelector(".settings-label")?.textContent).toBe("Base command");
      expect(baseInput.value).toBe('codex --profile "x y"');
      const cardInput = q<HTMLTextAreaElement>(rendered.root, `${CARD}.command`)!;
      expect(cardInput.value).toBe("--legacy");
      const labels = Array.from(rendered.root.querySelectorAll(".settings-profile-field-label"))
        .map((label) => label.textContent);
      expect(labels).toContain("Additional arguments");
      expect(labels).not.toContain("Command");
      expect(q(rendered.root, `${CARD}.commandBase`)?.textContent)
        .toContain("If left empty, a previously saved legacy value still applies.");
    } finally {
      rendered.cleanup();
    }
  });

  it("saves edited base text and effective cells under canonical keys only", async () => {
    const { fake, rendered } = renderWire();
    try {
      await ready(rendered.root);
      const baseInput = q<HTMLInputElement>(rendered.root, "settings.agentRow.0.command")!;
      baseInput.value = 'wrapper "C:/Program Files/codex.exe" --fixed';
      baseInput.dispatchEvent(new Event("input", { bubbles: true }));
      await expectFooterError(rendered.root, "");
      q<HTMLButtonElement>(rendered.root, "settings.save")!.click();
      await waitFor(() => expect(fake.callsFor("save_settings_draft")).toHaveLength(1));
      const draft = JSON.parse(JSON.stringify(fake.lastCall("save_settings_draft")!.args.draft));
      expect(draft.agents).toHaveLength(1);
      expect(draft.agents[0].baseCommand).toBe('wrapper "C:/Program Files/codex.exe" --fixed');
      expect(Object.keys(draft.agents[0])).not.toContain("command");
      const cells = draft.codingAgentProfiles.profilesByAgent.codex;
      expect(cells.A).toEqual({ enabled: true, env: {}, notes: "", additionalArguments: "--legacy" });
      expect(cells.B).toEqual({ enabled: false, env: {}, notes: "", additionalArguments: "--new-off" });
    } finally {
      rendered.cleanup();
    }
  });
});
