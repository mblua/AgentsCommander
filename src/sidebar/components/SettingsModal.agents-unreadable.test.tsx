// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import AgentPickerModal from "./AgentPickerModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import type { AgentConfig } from "../../shared/types";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";

// #2716 (B3, option C) - an unreadable agents file reaches the UI only through
// the snapshot's two read-only fields, and both surfaces reuse the
// `overlayOwnsAgents` disable-plus-explain pattern with notice 2.

const AGENTS_FILE = "C:/cfg/agents.30.instance.no-git.json";
const NOTICE_2 = `Unavailable: AgentsCommander could not read ${AGENTS_FILE}. Fix or delete that file and restart AgentsCommander. While this file is unreadable, changes to your sessions are not saved.`;

function unreadableFake(unreadable: boolean, agents: AgentConfig[] = []): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", {
    ...settingsSnapshot({ agents }),
    agentsLayerUnreadable: unreadable,
    agentsFilePath: unreadable ? AGENTS_FILE : null,
  });
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  return fake;
}

const byTestId = (root: HTMLElement, id: string) =>
  root.querySelector<HTMLElement>(`[data-ac-testid="${id}"]`);

describe("unreadable agents file notice (#2716)", () => {
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

  it("the Coding Agents tab shows notice 2, with its fourth sentence", async () => {
    const r = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      unreadableFake(true)
    );
    try {
      await waitFor(() =>
        expect(byTestId(r.root, "settings.agents.unreadableReason")?.textContent).toBe(NOTICE_2)
      );
    } finally {
      r.cleanup();
    }
  });

  it("the Coding Agents tab shows no notice while the file is readable", async () => {
    const r = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      unreadableFake(false)
    );
    try {
      await waitFor(() => expect(byTestId(r.root, "settings.agents.moveStatus")).toBeTruthy());
      expect(byTestId(r.root, "settings.agents.unreadableReason")).toBeNull();
    } finally {
      r.cleanup();
    }
  });

  it("the picker shows notice 2, with its fourth sentence, and disables reordering", async () => {
    const kept: AgentConfig = {
      id: "codex",
      label: "Codex",
      command: "codex",
      color: "#10b981",
      envs: [],
      isolatedHome: false,
    };
    const r = renderWithFakeTransport(
      () => <AgentPickerModal sessionName="architect" onSelect={() => {}} onClose={() => {}} />,
      unreadableFake(true, [kept])
    );
    try {
      await waitFor(() =>
        expect(byTestId(r.root, "agentPicker.unreadableReason")?.textContent).toBe(NOTICE_2)
      );
      const grip = byTestId(r.root, "agentPicker.provider.codex.dragHandle") as HTMLButtonElement;
      expect(grip).toBeTruthy();
      expect(grip.disabled).toBe(true);
    } finally {
      r.cleanup();
    }
  });
});
