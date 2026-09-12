// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { codingAgentsStore } from "../stores/coding-agents";
import type {
  AgentConfig,
  CatalogDiagnostic,
  CatalogReport,
  CodingAgentDefinition,
} from "../../shared/types";

const REPORT_CMD = "get_coding_agent_catalog_report";

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

function warning(code: string, path: string, reason: string): CatalogDiagnostic {
  return { code, path, reason };
}

/** A success report carries all five fields; overrides exercise the rest. */
function report(overrides: Partial<CatalogReport> = {}): CatalogReport {
  return {
    primaryProjectRoot: null,
    sourcePath: null,
    catalog: [],
    warnings: [],
    unavailable: null,
    ...overrides,
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

function byTestId<T extends HTMLElement = HTMLElement>(
  root: HTMLElement,
  testId: string,
): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function presetBtn(root: HTMLElement, key: string): HTMLButtonElement | null {
  return root.querySelector<HTMLButtonElement>(`[data-ac-testid="settings.agentPreset.${key}"]`);
}

function presetButtons(root: HTMLElement): HTMLButtonElement[] {
  return Array.from(
    root.querySelectorAll<HTMLButtonElement>('[data-ac-testid^="settings.agentPreset."]'),
  );
}

function tick(): Promise<void> {
  return new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("SettingsModal coding-agent quick-add row (#1965 catalog report)", () => {
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
    fake.resolve(
      REPORT_CMD,
      report({
        primaryProjectRoot: null,
        catalog: [
          def("claude", "Claude Code", "claude"),
          def("codex", "Codex", "codex"),
          def("pi", "Pi", "pi"),
        ],
      }),
    );

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(presetBtn(rendered.root, "codex")).toBeTruthy());

      // Exactly the three fetched keys, in file order (custom stays separate).
      const keys = presetButtons(rendered.root).map((el) => el.getAttribute("data-ac-testid"));
      expect(keys).toEqual([
        "settings.agentPreset.claude",
        "settings.agentPreset.codex",
        "settings.agentPreset.pi",
      ]);

      expect(presetBtn(rendered.root, "claude")!.disabled).toBe(true);
      expect(presetBtn(rendered.root, "codex")!.disabled).toBe(false);
      expect(presetBtn(rendered.root, "pi")!.disabled).toBe(false);
      // The hardcoded Custom Agent button is still present, last.
      expect(byTestId(rendered.root, "settings.agent.addCustom")).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows an unavailable report's path/reason, no fallback presets, and a usable Custom Agent", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve(
      REPORT_CMD,
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        unavailable: warning(
          "baseUnavailable",
          "C:/repo/app/.ac/coding-agents/agents.json",
          "catalog bytes are corrupt",
        ),
      }),
    );

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.error")).toBeTruthy());
      const errorArea = byTestId(rendered.root, "settings.catalog.error")!;
      expect(errorArea.textContent).toContain("Catalog unavailable");
      expect(byTestId(rendered.root, "settings.catalog.error.path")!.textContent).toContain(
        "C:/repo/app/.ac/coding-agents/agents.json",
      );
      expect(byTestId(rendered.root, "settings.catalog.error.reason")!.textContent).toContain(
        "catalog bytes are corrupt",
      );

      // Never a silent built-in fallback, and no re-seed controls without a catalog.
      expect(presetButtons(rendered.root)).toEqual([]);
      expect(byTestId(rendered.root, "settings.agent.reseedDefault.claude")).toBeNull();

      // Manual creation stays available.
      const custom = byTestId<HTMLButtonElement>(rendered.root, "settings.agent.addCustom")!;
      custom.click();
      await waitFor(() => expect(byTestId(rendered.root, "settings.agentRow.0")).toBeTruthy());
    } finally {
      rendered.cleanup();
    }
  });

  it("renders a warning's path/reason while the base rows stay selectable", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve(
      REPORT_CMD,
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        catalog: [def("claude", "Claude Code", "claude")],
        warnings: [
          warning(
            "local-overlay-invalid",
            "C:/repo/app/.ac/coding-agents/agents.local.json",
            "unknown field",
          ),
        ],
      }),
    );

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.warning.0")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.warning.0.path")!.textContent).toContain(
        "agents.local.json",
      );
      expect(byTestId(rendered.root, "settings.catalog.warning.0.reason")!.textContent).toContain(
        "unknown field",
      );

      // The readable base row is still offered and usable.
      const button = presetBtn(rendered.root, "claude")!;
      expect(button.disabled).toBe(false);
      button.click();
      await waitFor(() => expect(byTestId(rendered.root, "settings.agentRow.0")).toBeTruthy());
    } finally {
      rendered.cleanup();
    }
  });

  it("recovers through Reload after a transport failure", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.reject(REPORT_CMD, "config-dir failure");

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.error")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.error.reason")!.textContent).toContain(
        "config-dir failure",
      );
      expect(presetButtons(rendered.root)).toEqual([]);

      fake.resolve(
        REPORT_CMD,
        report({ primaryProjectRoot: null, catalog: [def("codex", "Codex", "codex")] }),
      );
      byTestId<HTMLButtonElement>(rendered.root, "settings.catalog.reload")!.click();

      await waitFor(() => expect(presetBtn(rendered.root, "codex")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.error")).toBeNull();
      expect(presetBtn(rendered.root, "codex")!.disabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("says so for a valid empty catalog and keeps Custom Agent usable", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [] }));

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.empty")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.empty")!.textContent).toContain(
        "No catalog agents available",
      );
      expect(presetButtons(rendered.root)).toEqual([]);
      expect(byTestId(rendered.root, "settings.agent.addCustom")).toBeTruthy();
      expect(byTestId(rendered.root, "settings.catalog.error")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the loading state until the report settles", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve("list_reseedable_agent_commands", []);
    let resolveReport!: (value: CatalogReport) => void;
    const pendingReport = new Promise<CatalogReport>((resolve) => {
      resolveReport = resolve;
    });
    fake.onInvoke(REPORT_CMD, () => pendingReport);

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.loading")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.loading")!.textContent).toContain(
        "Loading catalog",
      );
      expect(presetButtons(rendered.root)).toEqual([]);

      resolveReport(
        report({ primaryProjectRoot: null, catalog: [def("codex", "Codex", "codex")] }),
      );
      await waitFor(() => expect(presetBtn(rendered.root, "codex")).toBeTruthy());
      expect(byTestId(rendered.root, "settings.catalog.loading")).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("a reload detaches the old preset row set before it can register", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve(
      REPORT_CMD,
      report({ primaryProjectRoot: null, catalog: [def("alpha", "Alpha", "alpha")] }),
    );

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(presetBtn(rendered.root, "alpha")).toBeTruthy());
      const staleButton = presetBtn(rendered.root, "alpha")!;

      // Same key and command, but a new catalog generation: Solid replaces the
      // row synchronously, so the old node is detached before it can be clicked.
      fake.resolve(
        REPORT_CMD,
        report({ primaryProjectRoot: null, catalog: [def("alpha", "Alpha", "alpha")] }),
      );
      await codingAgentsStore.refresh();

      expect(staleButton.isConnected).toBe(false);
      staleButton.click();
      await tick();

      expect(byTestId(rendered.root, "settings.agentRow.0")).toBeNull();
      // The fresh generation's own row remains selectable.
      expect(presetBtn(rendered.root, "alpha")!.disabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("the add handler revalidates generation and definition availability (defense-in-depth)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.resolve(
      REPORT_CMD,
      report({ primaryProjectRoot: null, catalog: [def("alpha", "Alpha", "alpha")] }),
    );

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      await waitFor(() => expect(presetBtn(rendered.root, "alpha")).toBeTruthy());
      const button = presetBtn(rendered.root, "alpha")!;

      // A row captured in an older generation must not write. Ordinary clicks
      // cannot reach this (the reload detaches the row first), so the store
      // reads are pinned to exercise the guard directly.
      const generation = codingAgentsStore.generation();
      const generationSpy = vi
        .spyOn(codingAgentsStore, "generation")
        .mockReturnValue(generation + 1);
      button.click();
      await tick();
      generationSpy.mockRestore();
      expect(byTestId(rendered.root, "settings.agentRow.0")).toBeNull();

      // Same generation, but the definition is no longer in the catalog.
      const catalogSpy = vi.spyOn(codingAgentsStore, "catalog").mockReturnValue([]);
      button.click();
      await tick();
      catalogSpy.mockRestore();
      expect(byTestId(rendered.root, "settings.agentRow.0")).toBeNull();

      // Control: with the real store reads the same click registers, so the
      // blocks above came from the guard and not from a dead button.
      button.click();
      await waitFor(() => expect(byTestId(rendered.root, "settings.agentRow.0")).toBeTruthy());
    } finally {
      rendered.cleanup();
    }
  });

  it("shows the Auto-update error note while the catalog is unavailable (#1551)", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings());
    fake.resolve("get_web_server_status", false);
    fake.reject(REPORT_CMD, "config-dir failure");
    fake.reject("get_agent_update_overview", "overview failure");

    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="agents" onClose={() => {}} />,
      fake,
    );
    try {
      const errorNote = () => byTestId(rendered.root, "settings.autoUpdate.error");
      await waitFor(() => expect(errorNote()).toBeTruthy());
      expect(errorNote()!.textContent).toContain("Auto-update status unavailable: ");
      expect(errorNote()!.textContent).toContain("overview failure");
      // never an empty table, never the loading note
      expect(byTestId(rendered.root, "settings.autoUpdate.list")).toBeNull();
      expect(byTestId(rendered.root, "settings.autoUpdate.loading")).toBeNull();
      // the two surfaces are independent: the preset row still reports the
      // catalog failure instead of silently falling back to built-ins.
      await waitFor(() => expect(byTestId(rendered.root, "settings.catalog.error")).toBeTruthy());
      expect(presetButtons(rendered.root)).toEqual([]);
      expect(fake.listensFor("agent_install_state_changed")).toHaveLength(1);
      expect(fake.listensFor("agent_updates_finished")).toHaveLength(1);
    } finally {
      rendered.cleanup();
    }
  });
});
