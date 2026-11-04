// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings, installBrowserDomStubs, renderWithFakeTransport, resetUiStoresForTests, waitFor,
} from "../../shared/testing/ui-harness";
import type { AgentConfig, ProfileCellConfig } from "../../shared/types";

// #3516 - the Preflight blocks inside Settings: save round-trip, key removal,
// invalid timeout drafts that block Save even when collapsed, refetch survival,
// and Escape ownership while the test panel is open.

const TIMEOUT_ERROR = "Preflight timeout: Enter a whole number of seconds from 1 to 4294967295.";
const AGENT = "settings.agentRow.0";
const CARD = "settings.profileCard.0.A";

function agent(id: string, over: Partial<AgentConfig> = {}): AgentConfig {
  return {
    id, label: id, command: id, color: "#334155", envs: [], isolatedHome: false, ...over,
  };
}

function render(agents: AgentConfig[], cellA: Partial<ProfileCellConfig> = {}) {
  const base = baseSettings({ agents });
  const settings = baseSettings({
    agents,
    codingAgentProfiles: {
      ...base.codingAgentProfiles,
      profilesByAgent: {
        ...base.codingAgentProfiles.profilesByAgent,
        codex: { A: { enabled: true, command: "", env: {}, notes: "", ...cellA } },
      },
    },
  });
  const fake = new FakeTransport();
  fake.resolve("get_settings", settings);
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  fake.resolve("save_settings_draft", null);
  const onClose = vi.fn();
  const rendered = renderWithFakeTransport(
    () => <SettingsModal section="agents" onClose={onClose} />,
    fake,
  );
  return { fake, rendered, root: rendered.root, onClose };
}

function q<T extends HTMLElement = HTMLElement>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function type(root: HTMLElement, testId: string, value: string) {
  const el = q<HTMLInputElement>(root, testId)!;
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

const footerError = (root: HTMLElement) =>
  root.querySelector(".modal-save-error")?.textContent ?? "";

async function openAgent(root: HTMLElement) {
  await waitFor(() => expect(q(root, `${AGENT}.toggle`)).toBeTruthy());
  if (!q(root, `${AGENT}.preflight`)) q(root, `${AGENT}.toggle`)!.click();
  await waitFor(() => expect(q(root, `${AGENT}.preflight.command`)).toBeTruthy());
  await waitFor(() => expect(q(root, `${CARD}.preflight.command`)).toBeTruthy());
}

async function savedDraft(r: ReturnType<typeof render>) {
  await waitFor(() => expect(footerError(r.root)).toBe(""));
  q<HTMLButtonElement>(r.root, "settings.save")!.click();
  await waitFor(() => expect(r.fake.callsFor("save_settings_draft")).toHaveLength(1));
  const draft = JSON.parse(JSON.stringify(r.fake.lastCall("save_settings_draft")!.args.draft));
  return { row: draft.agents[0], cell: draft.codingAgentProfiles.profilesByAgent.codex.A };
}

describe("SettingsModal preflight blocks (#3516)", () => {
  let cleanupDom: (() => void) | null = null;
  let cleanup: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanup?.();
    cleanup = null;
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function start(...args: Parameters<typeof render>) {
    const r = render(...args);
    cleanup = r.rendered.cleanup;
    return r;
  }

  it("round-trips the command and timeout on an agent and on a cell", async () => {
    const r = start([agent("codex")]);
    await openAgent(r.root);
    expect(q<HTMLDetailsElement>(r.root, `${AGENT}.preflight`)!.open).toBe(false);
    type(r.root, `${AGENT}.preflight.command`, "check-agent");
    type(r.root, `${AGENT}.preflight.timeout`, " 90 ");
    type(r.root, `${CARD}.preflight.command`, "check-cell");
    type(r.root, `${CARD}.preflight.timeout`, "5");
    const { row, cell } = await savedDraft(r);
    expect(row).toMatchObject({ preflight: "check-agent", preflightTimeoutSeconds: 90 });
    expect(cell).toMatchObject({ preflight: "check-cell", preflightTimeoutSeconds: 5 });
  });

  it("removes both keys when a saved preflight is cleared and the timeout reset", async () => {
    const saved = { preflight: "old", preflightTimeoutSeconds: 30 };
    const r = start([agent("codex", saved)], saved);
    await openAgent(r.root);
    expect(q<HTMLInputElement>(r.root, `${AGENT}.preflight.timeout`)!.value).toBe("30");
    type(r.root, `${AGENT}.preflight.command`, "  ");
    q(r.root, `${AGENT}.preflight.timeoutReset`)!.click();
    type(r.root, `${CARD}.preflight.command`, "");
    type(r.root, `${CARD}.preflight.timeout`, "");
    const { row, cell } = await savedDraft(r);
    for (const value of [row, cell]) {
      expect(Object.keys(value)).not.toContain("preflight");
      expect(Object.keys(value)).not.toContain("preflightTimeoutSeconds");
    }
  });

  it("blocks Save on an invalid timeout, also when collapsed, until it is valid", async () => {
    const r = start([agent("codex")]);
    await openAgent(r.root);
    type(r.root, `${AGENT}.preflight.timeout`, "0");
    await waitFor(() => expect(footerError(r.root)).toBe(TIMEOUT_ERROR));
    const input = q<HTMLInputElement>(r.root, `${AGENT}.preflight.timeout`)!;
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent)
      .toBe("Enter a whole number of seconds from 1 to 4294967295.");
    expect(q<HTMLButtonElement>(r.root, "settings.save")!.disabled).toBe(true);
    q(r.root, `${AGENT}.toggle`)!.click();
    await waitFor(() => expect(q(r.root, `${AGENT}.preflight`)).toBeNull());
    expect(footerError(r.root)).toBe(TIMEOUT_ERROR);
    q(r.root, `${AGENT}.toggle`)!.click();
    await waitFor(() => expect(q(r.root, `${AGENT}.preflight.timeout`)).toBeTruthy());
    expect(q<HTMLInputElement>(r.root, `${AGENT}.preflight.timeout`)!.value).toBe("0");
    type(r.root, `${AGENT}.preflight.timeout`, "12");
    const { row } = await savedDraft(r);
    expect(row.preflightTimeoutSeconds).toBe(12);
  });

  it("clears the draft key of a removed agent", async () => {
    const r = start([agent("codex"), agent("claude")]);
    await waitFor(() => expect(q(r.root, "settings.agentRow.1.toggle")).toBeTruthy());
    q(r.root, "settings.agentRow.1.toggle")!.click();
    await waitFor(() => expect(q(r.root, "settings.agentRow.1.preflight.timeout")).toBeTruthy());
    type(r.root, "settings.agentRow.1.preflight.timeout", "abc");
    await waitFor(() => expect(footerError(r.root)).toBe(TIMEOUT_ERROR));
    q(r.root, "settings.agentRow.1.remove")!.click();
    await waitFor(() => expect(footerError(r.root)).toBe(""));
    expect(q<HTMLButtonElement>(r.root, "settings.save")!.disabled).toBe(false);
  });

  it("keeps a typed preflight across a coding_agent_settings_updated refetch", async () => {
    const r = start([agent("codex")]);
    await openAgent(r.root);
    type(r.root, `${AGENT}.preflight.command`, "typed");
    const gets = r.fake.callsFor("get_settings").length;
    r.fake.emitFromBackend("coding_agent_settings_updated", {});
    await waitFor(() => expect(r.fake.callsFor("get_settings").length).toBeGreaterThan(gets));
    await waitFor(() => expect(q<HTMLInputElement>(r.root, `${AGENT}.preflight.command`)!.value)
      .toBe("typed"));
    const { row } = await savedDraft(r);
    expect(row.preflight).toBe("typed");
  });

  it("closes only the test panel on Escape and keeps Settings open", async () => {
    const r = start([agent("codex")]);
    await openAgent(r.root);
    const opener = q<HTMLButtonElement>(r.root, `${AGENT}.preflight.test`)!;
    opener.click();
    await waitFor(() => expect(q(r.root, "settings.preflightTest")).toBeTruthy());
    const escape = () => document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    escape();
    await waitFor(() => expect(q(r.root, "settings.preflightTest")).toBeNull());
    expect(r.onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(opener);
    escape();
    await waitFor(() => expect(r.onClose).toHaveBeenCalled());
  });
});
