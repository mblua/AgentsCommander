// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setAlwaysOnTop: async () => {} }),
}));
// The Export/Import buttons render only on desktop; each test picks the platform.
const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  isWindows: false,
  get isBrowser() { return !platform.tauri; },
  get isTauri() { return platform.tauri; },
}));

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
import type { AgentConfig } from "../../shared/types";
import { buildTransferFile } from "../../shared/coding-agents-transfer";

const EXPORT_CMD = "coding_agents_export_pick_save";
const IMPORT_CMD = "coding_agents_import_pick_open";

function agent(id: string, label: string, value = ""): AgentConfig {
  const envs = value ? [{ key: "K", value, source: "user" as const, enabled: true }] : [];
  return { id, label, command: "claude", color: "#334155", envs, isolatedHome: false };
}

function makeFake(agents: AgentConfig[]): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings({ agents }));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog_report", {
    primaryProjectRoot: null, sourcePath: null, catalog: [], warnings: [], unavailable: null,
  });
  fake.resolve("list_reseedable_agent_commands", []);
  return fake;
}

function render(fake: FakeTransport) {
  return renderWithFakeTransport(() => <SettingsModal section="agents" onClose={() => {}} />, fake);
}

const btn = (root: HTMLElement, name: string) =>
  root.querySelector<HTMLButtonElement>(`[data-ac-testid="settings.agents.${name}"]`);

function labels(root: HTMLElement): string[] {
  return [...root.querySelectorAll('[data-ac-testid$=".dragHandle"]')].map((el) =>
    (el.getAttribute("aria-label") ?? "").replace(/^Reorder (.*), position.*$/, "$1"),
  );
}

const tick = () => new Promise((r) => setTimeout(r, 20));

describe("SettingsModal coding agents export/import (#5071)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    platform.tauri = true;
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    vi.restoreAllMocks();
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("shows both buttons on desktop and neither in the browser", async () => {
    const r = render(makeFake([agent("a1", "Claude")]));
    try {
      await waitFor(() => expect(btn(r.root, "export")).toBeTruthy());
      expect(btn(r.root, "import")?.textContent).toBe("Import");
    } finally {
      r.cleanup();
    }
    platform.tauri = false;
    const b = render(makeFake([agent("a1", "Claude")]));
    try {
      await waitFor(() => expect(labels(b.root)).toEqual(["Claude"]));
      expect(btn(b.root, "export")).toBeNull();
      expect(btn(b.root, "import")).toBeNull();
    } finally {
      b.cleanup();
    }
  });

  it("exports all agents and warns about env values", async () => {
    const fake = makeFake([agent("a1", "Claude", "secret"), agent("a2", "Codex")]);
    fake.resolve(EXPORT_CMD, "/tmp/out.json");
    const error = vi.spyOn(toastStore, "error");
    const r = render(fake);
    try {
      await waitFor(() => expect(labels(r.root)).toHaveLength(2));
      btn(r.root, "export")?.click();
      await waitFor(() => expect(toastStore.items).toHaveLength(1));
      const args = fake.lastCall(EXPORT_CMD)?.args as { content: string; defaultFileName: string };
      const sent = JSON.parse(args.content);
      expect(sent.agents.map((e: { agent: AgentConfig }) => e.agent.label)).toEqual([
        "Claude", "Codex",
      ]);
      expect(args.defaultFileName).toMatch(/^ac-coding-agents-exports_\d{8}_\d{6}\.json$/);
      expect(toastStore.items[0].kind).toBe("error");
      expect(toastStore.items[0].message).toBe(
        "Exported 2 coding agents to /tmp/out.json." +
          " The file contains environment variable values; share it carefully.",
      );
      expect(error).toHaveBeenCalledWith(toastStore.items[0].message);
    } finally {
      r.cleanup();
    }
  });

  it("exports without env values with a success toast", async () => {
    const fake = makeFake([agent("a1", "Claude")]);
    fake.resolve(EXPORT_CMD, "/tmp/out.json");
    const r = render(fake);
    try {
      await waitFor(() => expect(btn(r.root, "export")).toBeTruthy());
      btn(r.root, "export")?.click();
      await waitFor(() => expect(toastStore.items).toHaveLength(1));
      expect(toastStore.items[0].kind).toBe("success");
      expect(toastStore.items[0].message).toBe("Exported 1 coding agents to /tmp/out.json.");
    } finally {
      r.cleanup();
    }
  });

  it("imports into the draft only; Cancel discards", async () => {
    const fake = makeFake([agent("a1", "Claude")]);
    const file = buildTransferFile([agent("x", "Claude")], baseSettings().codingAgentProfiles,
      new Date());
    fake.resolve(IMPORT_CMD, JSON.stringify(file));
    const r = render(fake);
    try {
      await waitFor(() => expect(labels(r.root)).toEqual(["Claude"]));
      btn(r.root, "import")?.click();
      await waitFor(() => expect(labels(r.root)).toEqual(["Claude", "Claude (1)"]));
      expect(toastStore.items.some((t) => t.kind === "success")).toBe(true);
      expect(fake.callsFor("save_settings_draft")).toHaveLength(0);
      r.root.querySelector<HTMLButtonElement>('[data-ac-testid="settings.cancel"]')?.click();
      await tick();
    } finally {
      r.cleanup();
    }
    expect(fake.callsFor("save_settings_draft")).toHaveLength(0);
    const again = render(fake);
    try {
      await waitFor(() => expect(labels(again.root)).toEqual(["Claude"]));
    } finally {
      again.cleanup();
    }
  });

  it("shows an error for an invalid file and changes nothing", async () => {
    const fake = makeFake([agent("a1", "Claude")]);
    fake.resolve(IMPORT_CMD, "not json");
    const r = render(fake);
    try {
      await waitFor(() => expect(labels(r.root)).toEqual(["Claude"]));
      btn(r.root, "import")?.click();
      await waitFor(() => expect(toastStore.items.some((t) => t.kind === "error")).toBe(true));
      expect(toastStore.items[0].message).toContain("Import failed: Import file is not valid JSON");
      expect(labels(r.root)).toEqual(["Claude"]);
    } finally {
      r.cleanup();
    }
  });

  it("does nothing when either dialog is cancelled", async () => {
    const fake = makeFake([agent("a1", "Claude", "v")]);
    fake.resolve(EXPORT_CMD, null);
    fake.resolve(IMPORT_CMD, null);
    const r = render(fake);
    try {
      await waitFor(() => expect(labels(r.root)).toEqual(["Claude"]));
      btn(r.root, "export")?.click();
      await waitFor(() => expect(fake.callsFor(EXPORT_CMD)).toHaveLength(1));
      await waitFor(() => expect(btn(r.root, "import")?.disabled).toBe(false));
      btn(r.root, "import")?.click();
      await waitFor(() => expect(fake.callsFor(IMPORT_CMD)).toHaveLength(1));
      await tick();
      expect(toastStore.items).toHaveLength(0);
      expect(labels(r.root)).toEqual(["Claude"]);
    } finally {
      r.cleanup();
    }
  });

  it("drops the import result after the modal unmounts", async () => {
    const fake = makeFake([agent("a1", "Claude")]);
    let finish: (text: string) => void = () => {};
    fake.onInvoke(IMPORT_CMD, () => new Promise<string>((res) => (finish = res)));
    const r = render(fake);
    await waitFor(() => expect(btn(r.root, "import")).toBeTruthy());
    btn(r.root, "import")?.click();
    await waitFor(() => expect(fake.callsFor(IMPORT_CMD)).toHaveLength(1));
    r.cleanup();
    finish("not json");
    await tick();
    expect(toastStore.items).toHaveLength(0);
  });

  it("drops a valid import result after the modal unmounts", async () => {
    const fake = makeFake([agent("a1", "Claude")]);
    const file = buildTransferFile([agent("x", "Codex")], baseSettings().codingAgentProfiles,
      new Date());
    let finish: (text: string) => void = () => {};
    fake.onInvoke(IMPORT_CMD, () => new Promise<string>((res) => (finish = res)));
    const r = render(fake);
    await waitFor(() => expect(btn(r.root, "import")).toBeTruthy());
    btn(r.root, "import")?.click();
    await waitFor(() => expect(fake.callsFor(IMPORT_CMD)).toHaveLength(1));
    r.cleanup();
    finish(JSON.stringify(file));
    await tick();
    expect(toastStore.items).toHaveLength(0);
  });
});
