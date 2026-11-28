// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings, installBrowserDomStubs, renderWithFakeTransport, resetUiStoresForTests, waitFor,
} from "../../shared/testing/ui-harness";
import type { AgentConfig } from "../../shared/types";

const P01_MESSAGE =
  "DuplicateConfigurationIdentity: Coding Agent 'Codex 2' has the same command and envs " +
  "(ignoring order, case and spaces), so both would share the same profiles. " +
  "Change the command or the envs.";

function agent(id: string, label: string, command: string, id_hash: string): AgentConfig {
  return { id, label, command, color: "#334155", envs: [], isolatedHome: false, id_hash };
}

function render(agents: AgentConfig[], saveError?: string) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  if (saveError) fake.reject("save_settings_draft", saveError);
  else fake.resolve("save_settings_draft", null);
  const rendered = renderWithFakeTransport(
    () => <SettingsModal section="agents" onClose={() => {}} />,
    fake,
  );
  return { fake, rendered };
}

const q = (root: HTMLElement, id: string) =>
  root.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);
const warning = (root: HTMLElement, i: number) =>
  q(root, `settings.agentRow.${i}.sharedIdentity.warning`);

function rowState(root: HTMLElement): string[] {
  return [0, 1, 2].map((i) => {
    const row = q(root, `settings.agentRow.${i}`);
    const text = warning(root, i)?.textContent;
    const peers = text ? text.replace(/^Same command and envs as (.*?):.*$/s, "$1") : "-";
    return `${row?.dataset.acAgentId}:${peers}`;
  });
}

const tick = () => new Promise((r) => setTimeout(r, 20));

const DUPES = [
  agent("a", "Codex 2", "codex", "cid1:aa"),
  agent("b", "just codex", "codex", "cid1:aa"),
  agent("c", "Other", "other", "cid1:bb"),
];

describe("SettingsModal shared identity warning", () => {
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

  it("warns on both duplicate rows, naming the other, and not on the unique row", async () => {
    const { rendered } = render(DUPES);
    const root = rendered.root;
    try {
      await waitFor(() => expect(q(root, "settings.agentRow.2")).toBeTruthy());
      expect(warning(root, 0)?.textContent).toContain("Same command and envs as just codex");
      expect(warning(root, 1)?.textContent).toContain("Same command and envs as Codex 2");
      expect(warning(root, 2)).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("does not warn for the same command with a different id_hash", async () => {
    const { rendered } = render([
      agent("c1", "Claude", "claude", "cid1:c1"),
      agent("c2", "Claude 2", "claude", "cid1:c2"),
    ]);
    const root = rendered.root;
    try {
      await waitFor(() => expect(q(root, "settings.agentRow.1")).toBeTruthy());
      expect(root.querySelector('[data-ac-testid$=".sharedIdentity.warning"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the P01 refusal in the footer and keeps the modal open", async () => {
    const { fake, rendered } = render(DUPES, P01_MESSAGE);
    const root = rendered.root;
    try {
      await waitFor(() => expect(q(root, "settings.agentRow.0.select")).toBeTruthy());
      q(root, "settings.save")!.click();
      await waitFor(() => expect(fake.callsFor("save_settings_draft")).toHaveLength(1));
      await waitFor(() =>
        expect(root.querySelector(".modal-save-error")?.textContent ?? "").toContain(
          "Coding Agent 'Codex 2' has the same command and envs",
        ),
      );
      expect(q(root, "settings.save")).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("warning follows the displayed row during a keyboard pick-up", async () => {
    const { rendered } = render(DUPES);
    const root = rendered.root;
    try {
      await waitFor(() => expect(q(root, "settings.agentRow.2.dragHandle")).toBeTruthy());
      expect(rowState(root)).toEqual(["a:just codex", "b:Codex 2", "c:-"]);
      const handle = () =>
        root.querySelector<HTMLElement>('[data-ac-agent-id="c"] .settings-agent-drag-handle')!;
      q(root, "settings.agentRow.2.dragHandle")!.dispatchEvent(
        new KeyboardEvent("keydown", { key: " ", bubbles: true }),
      );
      await tick();
      handle().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
      await tick();
      handle().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
      await tick();
      expect(rowState(root)).toEqual(["c:-", "a:just codex", "b:Codex 2"]);
    } finally {
      rendered.cleanup();
    }
  });
});
