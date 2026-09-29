// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "solid-js/web";
import CodingAgentQuickConfiguration, { formatInstallOutput } from "./CodingAgentQuickConfiguration";
import type {
  AppSettings,
  CatalogReport,
  CodingAgentDefinition,
  CodingAgentInstallFinished,
  CodingAgentWelcomeStatus,
  SettingsSnapshot,
} from "../../shared/types";
import { SettingsAPI, CodingAgentsAPI, onCodingAgentInstallFinished } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { codingAgentsStore } from "../stores/coding-agents";
import { input as inputValue } from "../../shared/testing/ui-harness";
import { onboardingPendingSettings } from "../../shared/testing/base-settings";
import { declValue, scanRules, soleRuleBody } from "../styles/css-test-helpers";
import { readFileSync } from "node:fs";

// #1965 — the cards are driven by codingAgentsStore, which reads the catalog
// report (never a bundled fallback). Resolve a report carrying Codex, the
// preset this suite selects; tests override it per case.
function defaultReport(): CatalogReport {
  return {
    primaryProjectRoot: null,
    sourcePath: null,
    catalog: [
      {
        key: "codex",
        label: "Codex",
        description: "Coding Agent by OpenAI",
        color: "#10b981",
        command: "codex",
        instructionsFilename: "AGENTS.md",
        envs: [],
        isolatedHome: false,
        removable: true,
        updateCommands: [],
        autoUpdate: false,
      },
    ],
    warnings: [],
    unavailable: null,
  };
}

function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return onboardingPendingSettings(overrides);
}

vi.mock("../../shared/ipc", () => ({
  SettingsAPI: {
    get: vi.fn(() => Promise.resolve(settings())),
    update: vi.fn(() => Promise.resolve()),
  },
  // #2736 P5 - the install listener must resolve to a function.
  onCodingAgentInstallFinished: vi.fn(() => Promise.resolve(() => {})),
  CodingAgentsAPI: {
    install: vi.fn(() => Promise.resolve()),
    getCatalogReport: vi.fn(() => Promise.resolve(defaultReport())),
    listReseedableCommands: vi.fn(() => Promise.resolve([])),
    reseedDefault: vi.fn(() => Promise.resolve({ dest: "", backupPath: "" })),
    welcomeStatus: vi.fn(() => Promise.resolve([])),
  },
}));

vi.mock("../../shared/stores/settings", () => ({
  settingsStore: {
    refresh: vi.fn(),
  },
}));

function catalogDef(key: string, label: string, command: string): CodingAgentDefinition {
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

async function settle(): Promise<void> {
  await Promise.resolve();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function pressEscape(): void {
  document
    .querySelector('[data-ac-testid="onboarding.overlay"]')
    ?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
}

function cancelButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.cancel"]');
}

function byTestId<T extends HTMLElement = HTMLElement>(testId: string): T | null {
  return document.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function presetCards(): HTMLElement[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-ac-testid^="onboarding.agentPreset."]'),
  );
}

function confirmButton(): HTMLButtonElement | null {
  return byTestId<HTMLButtonElement>("onboarding.confirm");
}

function modalState(): string | null | undefined {
  return document
    .querySelector('[data-ac-testid="onboarding.modal"]')
    ?.getAttribute("data-ac-state");
}

async function selectCodexAndConfirm(): Promise<void> {
  document
    .querySelector<HTMLButtonElement>('[data-ac-testid="onboarding.agentPreset.codex"]')
    ?.click();
  await settle();
  confirmButton()?.click();
  await settle();
}

function renderModal(): () => void {
  const root = document.createElement("div");
  document.body.append(root);
  return render(
    () =>
      CodingAgentQuickConfiguration({
        title: "Add a Coding Agent",
        message: "Pick a Coding Agent to configure.",
        onClose: vi.fn(),
      }),
    root,
  );
}

describe("CodingAgentQuickConfiguration", () => {
  beforeEach(() => {
    codingAgentsStore.resetForTests();
  });

  afterEach(() => {
    document.body.innerHTML = "";
    vi.clearAllMocks();
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockImplementation(() =>
      Promise.resolve(defaultReport()),
    );
    vi.mocked(CodingAgentsAPI.listReseedableCommands).mockImplementation(() =>
      Promise.resolve([]),
    );
    vi.mocked(SettingsAPI.get).mockImplementation(() =>
      Promise.resolve(settings() as SettingsSnapshot),
    );
    vi.mocked(SettingsAPI.update).mockImplementation(() => Promise.resolve());
    vi.mocked(CodingAgentsAPI.welcomeStatus).mockImplementation(() => Promise.resolve([]));
    codingAgentsStore.resetForTests();
  });

  it("renders the consumer-provided title and message", async () => {
    const dispose = renderModal();
    await settle();

    expect(document.querySelector(".agent-modal-title")?.textContent).toBe("Add a Coding Agent");
    expect(document.querySelector(".onboarding-welcome")?.textContent).toBe(
      "Pick a Coding Agent to configure.",
    );
    // Accessible name falls back to the title when no override is supplied.
    expect(
      document.querySelector('[data-ac-testid="onboarding.modal"]')?.getAttribute("aria-label"),
    ).toBe("Add a Coding Agent");

    dispose();
  });

  it("shows the loading state until the report settles", async () => {
    let resolveReport!: (value: CatalogReport) => void;
    const pendingReport = new Promise<CatalogReport>((resolve) => {
      resolveReport = resolve;
    });
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockReturnValueOnce(pendingReport);

    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.loading")?.textContent).toContain("Loading catalog");

    resolveReport(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await vi.waitFor(() => expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy());
    expect(byTestId("onboarding.catalog.loading")).toBeNull();

    dispose();
  });

  it("renders no Cancel button when no cancel callback is supplied", async () => {
    const dispose = renderModal();
    await settle();

    expect(cancelButton()).toBeNull();
    // The confirm affordance still exists, so the modal is not a dead end.
    expect(confirmButton()).toBeTruthy();

    dispose();
  });

  it("renders Cancel and invokes the callback when supplied", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    const cancel = cancelButton();
    expect(cancel?.textContent).toBe("Cancel");

    cancel?.click();
    await settle();

    // The callback owns closing; the component must not close on its own.
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(SettingsAPI.update).not.toHaveBeenCalled();

    dispose();
  });

  it("routes Escape to the cancel callback when one is supplied", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    pressEscape();
    await settle();

    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();

    dispose();
  });

  it("leaves Escape inert when no cancel callback is supplied", async () => {
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onClose,
        }),
      root,
    );
    await settle();

    pressEscape();
    await settle();

    // No back-door dismissal: Escape must not close, and must not persist.
    expect(onClose).not.toHaveBeenCalled();
    expect(SettingsAPI.update).not.toHaveBeenCalled();
    expect(byTestId("onboarding.modal")).toBeTruthy();

    dispose();
  });

  it("confirms an agent without any onboarding side effect", async () => {
    const dispose = renderModal();
    await settle();

    await selectCodexAndConfirm();

    // #975 — the reusable component persists the agent and nothing else: it
    // must never flip onboardingDismissed on a consumer's behalf.
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: false,
        agents: [expect.objectContaining({ label: "Codex", command: "codex" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

    dispose();
  });

  it("folds onBeforeSave changes into the single agent write", async () => {
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onBeforeSave: (current) => ({ ...current, onboardingDismissed: true }),
          onClose: vi.fn(),
        }),
      root,
    );
    await settle();

    await selectCodexAndConfirm();

    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        onboardingDismissed: true,
        agents: [expect.objectContaining({ label: "Codex" })],
      }),
    );

    dispose();
  });

  it("closes on Escape in the success state without re-entering the cancel path (#975)", async () => {
    const onCancel = vi.fn();
    const onClose = vi.fn();
    const root = document.createElement("div");
    document.body.append(root);
    const dispose = render(
      () =>
        CodingAgentQuickConfiguration({
          title: "Add a Coding Agent",
          message: "Pick a Coding Agent to configure.",
          onCancel,
          onClose,
        }),
      root,
    );
    await settle();

    await selectCodexAndConfirm();
    expect(byTestId("onboarding.done")).toBeTruthy();

    pressEscape();
    await settle();

    // #975 F1 — the success footer renders only "Get started", so Escape must
    // resolve to onClose. Routing it to onCancel would re-run the consumer's
    // cancel path and issue a second settings write after a completed save.
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
    expect(SettingsAPI.get).toHaveBeenCalledTimes(1);
    expect(SettingsAPI.update).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("advances data-ac-state selecting -> done and refreshes the settings store", async () => {
    const dispose = renderModal();
    await settle();

    expect(modalState()).toBe("selecting");

    await selectCodexAndConfirm();

    // Automation state contract: consumers wait on done/selecting.
    expect(modalState()).toBe("done");
    // Without this refresh the sidebar keeps showing zero agents after setup.
    expect(settingsStore.refresh).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("shows the report path/reason for an unavailable catalog and offers no fallback cards", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        unavailable: {
          code: "baseUnavailable",
          path: "C:/repo/app/.ac/coding-agents/agents.json",
          reason: "catalog bytes are corrupt",
        },
      }),
    );
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error")?.textContent).toContain("Catalog unavailable");
    expect(byTestId("onboarding.catalog.error.path")?.textContent).toContain(
      "C:/repo/app/.ac/coding-agents/agents.json",
    );
    expect(byTestId("onboarding.catalog.error.reason")?.textContent).toContain(
      "catalog bytes are corrupt",
    );
    // Only the manual Custom Agent card exists; no selectable embedded defaults.
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    dispose();
  });

  it("shows a warning's path/reason while the readable base card stays selectable", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        catalog: [catalogDef("codex", "Codex", "codex")],
        warnings: [
          {
            code: "local-overlay-invalid",
            path: "C:/repo/app/.ac/coding-agents/agents.local.json",
            reason: "unknown field",
          },
        ],
      }),
    );
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.warning.0.path")?.textContent).toContain(
      "agents.local.json",
    );
    expect(byTestId("onboarding.catalog.warning.0.reason")?.textContent).toContain("unknown field");
    expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy();

    // The readable base row is usable.
    await selectCodexAndConfirm();
    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        agents: [expect.objectContaining({ label: "Codex", command: "codex" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

    dispose();
  });

  it("keeps Manual Custom Agent creation usable when the catalog is unavailable", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockRejectedValue("config-dir failure");
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error.reason")?.textContent).toContain(
      "config-dir failure",
    );
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    byTestId<HTMLButtonElement>("onboarding.agentPreset.custom")!.click();
    await settle();
    const label = byTestId<HTMLInputElement>("onboarding.custom.label")!;
    const command = byTestId<HTMLInputElement>("onboarding.custom.command")!;
    inputValue(label, "My Agent");
    inputValue(command, "my-agent --flag");
    await settle();

    confirmButton()!.click();
    await settle();

    expect(SettingsAPI.update).toHaveBeenCalledWith(
      expect.objectContaining({
        agents: [expect.objectContaining({ label: "My Agent", command: "my-agent --flag" })],
      }),
    );
    expect(byTestId("onboarding.done")).toBeTruthy();

    dispose();
  });

  it("recovers through Reload after a catalog failure (no fallback in between)", async () => {
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockRejectedValue("config-dir failure");
    const dispose = renderModal();
    await settle();

    expect(byTestId("onboarding.catalog.error")).toBeTruthy();
    expect(presetCards().map((el) => el.getAttribute("data-ac-testid"))).toEqual([
      "onboarding.agentPreset.custom",
    ]);

    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    byTestId<HTMLButtonElement>("onboarding.catalog.reload")!.click();

    await vi.waitFor(() => expect(byTestId("onboarding.agentPreset.codex")).toBeTruthy());
    expect(byTestId("onboarding.catalog.error")).toBeNull();
    expect(confirmButton()!.disabled).toBe(true); // nothing selected yet

    dispose();
  });

  it("disables a stale preset confirmation before the settings fetch", async () => {
    const dispose = renderModal();
    await settle();

    byTestId<HTMLButtonElement>("onboarding.agentPreset.codex")!.click();
    await settle();
    expect(confirmButton()!.disabled).toBe(false);

    // A reload invalidates the selection made in the previous generation.
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await codingAgentsStore.refresh();
    await settle();

    expect(confirmButton()!.disabled).toBe(true);
    expect(byTestId("onboarding.agentPreset.codex")!.getAttribute("data-ac-state")).toBe("idle");
    // Manual custom fields would survive; the catalog selection does not.

    dispose();
  });

  it("aborts a pending confirmation when the generation changes during the settings fetch", async () => {
    let resolveSettings!: (value: SettingsSnapshot) => void;
    const pendingSettings = new Promise<SettingsSnapshot>((resolve) => {
      resolveSettings = resolve;
    });
    vi.mocked(SettingsAPI.get).mockReturnValueOnce(pendingSettings);

    const dispose = renderModal();
    await settle();

    byTestId<HTMLButtonElement>("onboarding.agentPreset.codex")!.click();
    await settle();
    confirmButton()!.click();
    await settle();
    expect(confirmButton()!.getAttribute("data-ac-state")).toBe("saving");

    // The project switched while the settings read was still pending.
    vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
      report({ primaryProjectRoot: null, catalog: [catalogDef("codex", "Codex", "codex")] }),
    );
    await codingAgentsStore.refresh();
    await settle();

    resolveSettings(settings() as SettingsSnapshot);
    await settle();
    await settle();

    // The preset from the old generation must not register.
    expect(SettingsAPI.update).not.toHaveBeenCalled();
    expect(byTestId("onboarding.done")).toBeNull();
    expect(confirmButton()!.disabled).toBe(true);

    dispose();
  });

  describe("Welcome status chips (#2736)", () => {
    const welcomeCatalog = () => [
      catalogDef("codex", "Codex", "codex"),
      catalogDef("claude", "Claude Code", "claude"),
      catalogDef("pi", "Pi", "pi"),
      catalogDef("mine", "My Agent", "mine"),
    ];

    function statusRow(
      key: string,
      installed: boolean,
      testedLevel: CodingAgentWelcomeStatus["testedLevel"],
    ): CodingAgentWelcomeStatus {
      return { key, installed, testedLevel, installCommand: null };
    }

    const welcomeRows = (): CodingAgentWelcomeStatus[] => [
      statusRow("codex", false, "high"),
      statusRow("claude", true, "medium"),
      statusRow("pi", false, "low"),
      statusRow("mine", false, null),
    ];

    function renderWelcome(showInstallStatus: boolean | undefined): () => void {
      const root = document.createElement("div");
      document.body.append(root);
      return render(
        () =>
          CodingAgentQuickConfiguration({
            title: "Welcome",
            message: "Pick one.",
            onClose: vi.fn(),
            showInstallStatus,
          }),
        root,
      );
    }

    function useWelcomeCatalog(rows: CodingAgentWelcomeStatus[] = welcomeRows()): void {
      vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
        report({ catalog: welcomeCatalog() }),
      );
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockResolvedValue(rows);
    }

    const chipText = (key: string, chip: "status" | "tested") =>
      byTestId(`onboarding.agentPreset.${key}.${chip}`)?.textContent ?? null;
    // The chip testids share the `onboarding.agentPreset.` prefix, so select
    // the cards by role, not by the testid prefix `presetCards()` uses.
    const cardKeys = () =>
      Array.from(document.querySelectorAll('[data-ac-role="agent-preset"]')).map((el) =>
        el.getAttribute("data-ac-agent-key"),
      );
    const cardLabel = (key: string) =>
      byTestId(`onboarding.agentPreset.${key}`)?.getAttribute("aria-label");

    it("welcome_2736_renders_status_and_tested_chips_when_the_flag_is_on", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(true);
      await settle();

      expect(chipText("claude", "status")).toBe("Installed");
      expect(chipText("codex", "status")).toBe("Not installed");
      expect(chipText("codex", "tested")).toBe("Tested: High");
      expect(chipText("claude", "tested")).toBe("Tested: Medium");
      expect(chipText("pi", "tested")).toBe("Tested: Low");
      expect(byTestId("onboarding.agentPreset.claude.status")?.getAttribute("data-ac-state")).toBe(
        "installed",
      );
      expect(byTestId("onboarding.agentPreset.codex.status")?.getAttribute("data-ac-state")).toBe(
        "missing",
      );
      expect(byTestId("onboarding.agentPreset.pi.tested")?.getAttribute("data-ac-state")).toBe("low");

      dispose();
    });

    it("welcome_2736_renders_no_chip_when_the_flag_is_off", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(undefined);
      await settle();

      expect(cardKeys().length).toBe(5);
      for (const key of ["codex", "claude", "pi", "mine", "custom"]) {
        expect(byTestId(`onboarding.agentPreset.${key}.status`)).toBeNull();
        expect(byTestId(`onboarding.agentPreset.${key}.tested`)).toBeNull();
      }
      expect(document.querySelector(".onboarding-card-chips")).toBeNull();
      // Flag off: catalog order and no status fetch.
      expect(cardKeys()).toEqual(["codex", "claude", "pi", "mine", "custom"]);
      expect(CodingAgentsAPI.welcomeStatus).not.toHaveBeenCalled();

      dispose();
    });

    it("welcome_2736_custom_agent_shows_not_needed_and_no_tested_chip", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(true);
      await settle();

      expect(chipText("custom", "status")).toBe("Not needed");
      expect(byTestId("onboarding.agentPreset.custom.status")?.getAttribute("data-ac-state")).toBe(
        "not-needed",
      );
      expect(byTestId("onboarding.agentPreset.custom.tested")).toBeNull();

      dispose();
    });

    it("welcome_2736_a_user_authored_key_gets_no_tested_chip", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(true);
      await settle();

      expect(chipText("mine", "status")).toBe("Not installed");
      expect(byTestId("onboarding.agentPreset.mine.tested")).toBeNull();

      dispose();
    });

    it("welcome_2736_status_chip_precedes_the_tested_chip_in_source_order", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(true);
      await settle();

      const status = byTestId("onboarding.agentPreset.codex.status")!;
      const tested = byTestId("onboarding.agentPreset.codex.tested")!;
      const row = status.parentElement!;
      expect(tested.parentElement).toBe(row);
      expect(row.classList.contains("onboarding-card-chips")).toBe(true);
      const children = Array.from(row.children);
      expect(children.indexOf(status)).toBe(0);
      expect(children.indexOf(tested)).toBe(1);
      expect(status.compareDocumentPosition(tested) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

      dispose();
    });

    it("welcome_2736_card_order_is_installed_then_level_then_catalog_with_custom_last", async () => {
      useWelcomeCatalog([
        statusRow("codex", false, "high"),
        statusRow("claude", true, "medium"),
        statusRow("pi", true, "low"),
        statusRow("mine", false, null),
      ]);
      const dispose = renderWelcome(true);
      await settle();

      expect(cardKeys()).toEqual(["claude", "pi", "codex", "mine", "custom"]);

      dispose();
    });

    it("welcome_2736_an_ipc_failure_leaves_catalog_order_and_no_tested_chip", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      useWelcomeCatalog();
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockRejectedValue("status failure");
      const dispose = renderWelcome(true);
      await settle();

      expect(cardKeys()).toEqual(["codex", "claude", "pi", "mine", "custom"]);
      expect(document.querySelector(".onboarding-chip-tested")).toBeNull();
      expect(chipText("claude", "status")).toBe("Not installed");
      expect(consoleError).toHaveBeenCalled();
      expect(byTestId("onboarding.catalog.error")).toBeNull();

      await selectCodexAndConfirm();
      expect(SettingsAPI.update).toHaveBeenCalledTimes(1);
      expect(modalState()).toBe("done");

      consoleError.mockRestore();
      dispose();
    });

    it("welcome_2736_a_generation_change_refetches_the_status", async () => {
      useWelcomeCatalog([statusRow("codex", false, "high")]);
      const dispose = renderWelcome(true);
      await settle();

      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(1);
      expect(chipText("codex", "status")).toBe("Not installed");

      // A NEW array for the second call: republishing the same reference would
      // be a no-op signal write and make this assertion vacuous.
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockResolvedValue([
        statusRow("codex", true, "high"),
      ]);
      await codingAgentsStore.refresh();
      await settle();

      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(2);
      expect(chipText("codex", "status")).toBe("Installed");
      expect(cardKeys()[0]).toBe("codex");

      dispose();
    });

    it("welcome_2736_a_stale_generation_response_is_discarded", async () => {
      let resolveFirst!: (rows: CodingAgentWelcomeStatus[]) => void;
      useWelcomeCatalog();
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockReturnValueOnce(
        new Promise<CodingAgentWelcomeStatus[]>((resolve) => {
          resolveFirst = resolve;
        }),
      );
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockResolvedValue([statusRow("codex", false, "low")]);
      const dispose = renderWelcome(true);
      await settle();
      await codingAgentsStore.refresh();
      await settle();

      resolveFirst([statusRow("codex", true, "high")]);
      await settle();

      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(2);
      expect(chipText("codex", "status")).toBe("Not installed");
      expect(chipText("codex", "tested")).toBe("Tested: Low");

      dispose();
    });

    it("welcome_2736_a_stale_generation_rejection_is_discarded_without_logging", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      let rejectFirst!: (reason: unknown) => void;
      useWelcomeCatalog();
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockReturnValueOnce(
        new Promise<CodingAgentWelcomeStatus[]>((_resolve, reject) => {
          rejectFirst = reject;
        }),
      );
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockResolvedValue([
        statusRow("codex", true, "high"),
      ]);
      const dispose = renderWelcome(true);
      await settle();
      await codingAgentsStore.refresh();
      await settle();
      expect(chipText("codex", "status")).toBe("Installed");

      rejectFirst("stale failure");
      await settle();

      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(2);
      expect(chipText("codex", "status")).toBe("Installed");
      expect(chipText("codex", "tested")).toBe("Tested: High");
      expect(consoleError).not.toHaveBeenCalled();

      consoleError.mockRestore();
      dispose();
    });

    it("welcome_2736_chips_add_no_focusable_element", async () => {
      const focusableCount = () =>
        document
          .querySelector('[data-ac-testid="onboarding.modal"]')!
          .querySelectorAll(
            'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])',
          ).length;

      useWelcomeCatalog();
      const disposeOff = renderWelcome(undefined);
      await settle();
      const off = focusableCount();
      disposeOff();
      document.body.innerHTML = "";

      const disposeOn = renderWelcome(true);
      await settle();
      expect(document.querySelectorAll(".onboarding-chip").length).toBeGreaterThan(0);
      expect(focusableCount()).toBe(off);

      disposeOn();
    });

    it("welcome_2736_card_accessible_name_includes_the_status_and_tested_chips", async () => {
      useWelcomeCatalog([
        statusRow("codex", true, "high"),
        statusRow("claude", true, "medium"),
        statusRow("pi", false, "low"),
        statusRow("mine", false, null),
      ]);
      const dispose = renderWelcome(true);
      await settle();

      expect(cardLabel("codex")).toBe("Select Codex, Installed, Tested: High");
      expect(cardLabel("claude")).toBe("Select Claude Code, Installed, Tested: Medium");
      expect(cardLabel("mine")).toBe("Select My Agent, Not installed");
      expect(cardLabel("custom")).toBe("Select Custom Agent, Not needed");
      // The name and the chip text must not drift apart.
      for (const key of ["claude", "mine", "custom", "codex", "pi"]) {
        const card = byTestId(`onboarding.agentPreset.${key}`)!;
        const name = card.querySelector(".onboarding-card-name")!.textContent;
        const tested = chipText(key, "tested");
        const expected = `Select ${name}, ${chipText(key, "status")}${tested ? `, ${tested}` : ""}`;
        expect(cardLabel(key)).toBe(expected);
      }

      dispose();
    });

    it("welcome_2736_card_accessible_name_is_unchanged_when_the_flag_is_off", async () => {
      useWelcomeCatalog();
      const dispose = renderWelcome(false);
      await settle();

      expect(cardLabel("claude")).toBe("Select Claude Code");
      expect(cardLabel("mine")).toBe("Select My Agent");
      expect(cardLabel("custom")).toBe("Select Custom Agent");

      dispose();
    });
  });

  describe("Welcome install actions (#2736)", () => {
    const CODEX_CMD = "npm install -g @openai/codex";
    const MINE_CMD = "npm install -g mine-agent";

    const installCatalog = () => [
      catalogDef("codex", "Codex", "codex"),
      catalogDef("claude", "Claude Code", "claude"),
      catalogDef("pi", "Pi", "pi"),
      catalogDef("mine", "My Agent", "mine"),
    ];

    function row(
      key: string,
      installed: boolean,
      installCommand: string | null,
      testedLevel: CodingAgentWelcomeStatus["testedLevel"] = null,
    ): CodingAgentWelcomeStatus {
      return { key, installed, testedLevel, installCommand };
    }

    // codex: missing with a command; claude: installed with a command;
    // pi: missing with no command; mine: missing with a command.
    const installRows = (): CodingAgentWelcomeStatus[] => [
      row("codex", false, CODEX_CMD, "high"),
      row("claude", true, "npm install -g claude", "medium"),
      row("pi", false, null, "low"),
      row("mine", false, MINE_CMD),
    ];

    type NodeEventHost = {
      on(event: string, listener: (...args: unknown[]) => void): void;
      off(event: string, listener: (...args: unknown[]) => void): void;
    };

    let finishedHandler: ((payload: CodingAgentInstallFinished) => void) | null = null;
    let unlisten: ReturnType<typeof vi.fn>;

    beforeEach(() => {
      finishedHandler = null;
      // Like the real transport, an unlistened handler receives nothing more.
      unlisten = vi.fn(() => {
        finishedHandler = null;
      });
      vi.mocked(onCodingAgentInstallFinished).mockImplementation((callback) => {
        finishedHandler = callback;
        return Promise.resolve(unlisten as unknown as () => void);
      });
      vi.mocked(CodingAgentsAPI.install).mockImplementation(() => Promise.resolve());
      vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(
        report({ catalog: installCatalog() }),
      );
      vi.mocked(CodingAgentsAPI.welcomeStatus).mockResolvedValue(installRows());
    });

    afterEach(() => {
      delete (navigator as { clipboard?: unknown }).clipboard;
    });

    function renderInstall(
      showInstallStatus: boolean | undefined,
      onCancel?: () => void,
    ): () => void {
      const root = document.createElement("div");
      document.body.append(root);
      return render(
        () =>
          CodingAgentQuickConfiguration({
            title: "Welcome",
            message: "Pick one.",
            onClose: vi.fn(),
            onCancel,
            showInstallStatus,
          }),
        root,
      );
    }

    function stubClipboard(writeText: (text: string) => Promise<void>): void {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText },
      });
    }

    const card = (key: string) => byTestId<HTMLButtonElement>(`onboarding.agentPreset.${key}`)!;
    const copyButton = (key: string) =>
      byTestId<HTMLButtonElement>(`onboarding.agentPreset.${key}.copy`);
    const installButton = (key: string) =>
      byTestId<HTMLButtonElement>(`onboarding.agentPreset.${key}.install`);
    const commandText = (key: string) => byTestId(`onboarding.agentPreset.${key}.installCommand`);
    const failedLine = (key: string) => byTestId(`onboarding.agentPreset.${key}.installFailed`);
    const installRow = (key: string) =>
      card(key).parentElement!.querySelector(".onboarding-card-install");
    const cardKeys = () =>
      Array.from(document.querySelectorAll('[data-ac-role="agent-preset"]')).map((el) =>
        el.getAttribute("data-ac-agent-key"),
      );
    const modal = () => byTestId("onboarding.modal")!;
    const focusables = () =>
      Array.from(
        modal().querySelectorAll<HTMLElement>(
          'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])',
        ),
      );
    const statusAfterCodexInstall = (): CodingAgentWelcomeStatus[] => [
      row("codex", true, CODEX_CMD, "high"),
      row("claude", true, "npm install -g claude", "medium"),
      row("pi", false, null, "low"),
      row("mine", false, MINE_CMD),
    ];

    async function emitFinished(payload: CodingAgentInstallFinished): Promise<void> {
      finishedHandler?.(payload);
      await settle();
    }

    /** Mount with the flag on, flush the listen promise, and prove the gate opened. */
    async function mountReady(onCancel?: () => void): Promise<() => void> {
      const dispose = renderInstall(true, onCancel);
      await settle();
      expect(installButton("codex")?.getAttribute("data-ac-state")).toBe("idle");
      return dispose;
    }

    it("install_2736_shows_command_copy_and_install_only_for_a_missing_agent_with_a_command", async () => {
      const dispose = await mountReady();

      expect(commandText("codex")?.textContent).toBe(CODEX_CMD);
      expect(copyButton("codex")).not.toBeNull();
      expect(installButton("codex")).not.toBeNull();
      for (const key of ["claude", "pi", "custom"]) {
        expect(commandText(key)).toBeNull();
        expect(copyButton(key)).toBeNull();
        expect(installButton(key)).toBeNull();
      }

      dispose();
    });

    it("install_2736_renders_nothing_new_when_the_flag_is_off", async () => {
      const dispose = renderInstall(undefined);
      await settle();

      expect(cardKeys().length).toBe(5);
      for (const key of ["codex", "claude", "pi", "mine", "custom"]) {
        expect(commandText(key)).toBeNull();
        expect(copyButton(key)).toBeNull();
        expect(installButton(key)).toBeNull();
      }
      expect(onCodingAgentInstallFinished).not.toHaveBeenCalled();

      dispose();
    });

    it("install_2736_copy_writes_the_exact_command_to_the_clipboard", async () => {
      const writeText = vi.fn(() => Promise.resolve());
      stubClipboard(writeText);
      const dispose = await mountReady();

      copyButton("codex")!.click();
      await settle();

      expect(writeText).toHaveBeenCalledTimes(1);
      expect(writeText).toHaveBeenCalledWith(CODEX_CMD);

      dispose();
    });

    it("install_2736_copy_survives_a_throwing_clipboard_accessor", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        get() {
          throw new Error("clipboard is not available in a non-secure context");
        },
      });
      const dispose = await mountReady();

      copyButton("codex")!.click();
      await settle();
      expect(consoleError).toHaveBeenCalledTimes(1);
      expect(modal()).not.toBeNull();
      expect(copyButton("codex")).not.toBeNull();

      // The weaker case: the accessor works and writeText rejects.
      stubClipboard(() => Promise.reject(new Error("denied")));
      copyButton("codex")!.click();
      await settle();
      expect(consoleError).toHaveBeenCalledTimes(2);
      expect(modal()).not.toBeNull();

      consoleError.mockRestore();
      dispose();
    });

    it("install_2736_copy_does_not_change_the_card_selection", async () => {
      stubClipboard(() => Promise.resolve());
      const dispose = await mountReady();

      copyButton("codex")!.click();
      await settle();
      expect(card("codex").getAttribute("aria-pressed")).toBe("false");
      expect(confirmButton()?.disabled).toBe(true);

      card("codex").click();
      await settle();
      copyButton("codex")!.click();
      await settle();
      expect(card("codex").getAttribute("aria-pressed")).toBe("true");
      expect(confirmButton()?.disabled).toBe(false);

      dispose();
    });

    it("install_2736_install_click_invokes_the_backend_once_with_the_key", async () => {
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();

      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(1);
      expect(CodingAgentsAPI.install).toHaveBeenCalledWith("codex");

      dispose();
    });

    it("install_2736_button_is_aria_disabled_and_labelled_installing_while_running", async () => {
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();

      const button = installButton("codex")!;
      expect(button.getAttribute("aria-disabled")).toBe("true");
      expect(button.textContent).toBe("Installing...");
      expect(button.getAttribute("data-ac-state")).toBe("installing");
      expect(button.hasAttribute("disabled")).toBe(false);

      dispose();
    });

    it("install_2736_a_refused_invoke_re_enables_the_button_and_shows_the_failure_line", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      vi.mocked(CodingAgentsAPI.install).mockRejectedValue("already running");
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();

      const button = installButton("codex")!;
      expect(button.getAttribute("aria-disabled")).toBe("false");
      expect(button.getAttribute("data-ac-state")).toBe("idle");
      expect(button.textContent).toBe("Install");
      expect(failedLine("codex")?.textContent).toBe("Install failed; see the app log.");
      expect(consoleError).toHaveBeenCalled();

      consoleError.mockRestore();
      dispose();
    });

    it("install_2736_ok_true_event_refetches_status_and_flips_the_row_to_installed", async () => {
      // The second response is a NEW array: the same reference would be a no-op write.
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockResolvedValueOnce(statusAfterCodexInstall());
      const dispose = await mountReady();
      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(1);

      installButton("codex")!.click();
      await settle();
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });

      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(2);
      expect(byTestId("onboarding.agentPreset.codex.status")?.textContent).toBe("Installed");
      expect(installRow("codex")).toBeNull();
      expect(failedLine("codex")).toBeNull();

      dispose();
    });

    it("install_2736_ok_true_event_re_sorts_the_list", async () => {
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockResolvedValueOnce([
          row("codex", false, CODEX_CMD, "high"),
          row("claude", true, "npm install -g claude", "medium"),
          row("pi", false, null, "low"),
          row("mine", true, MINE_CMD),
        ]);
      const dispose = await mountReady();
      const before = cardKeys();
      expect(before.indexOf("mine")).toBeGreaterThan(before.indexOf("codex"));

      installButton("mine")!.click();
      await settle();
      await emitFinished({
        key: "mine",
        command: MINE_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });

      const after = cardKeys();
      expect(after).not.toEqual(before);
      expect(after.indexOf("mine")).toBeLessThan(after.indexOf("codex"));
      expect(after.indexOf("mine")).toBeLessThan(after.indexOf("pi"));

      dispose();
    });

    it("install_2736_ok_false_event_keeps_the_row_missing_and_shows_the_failure_line", async () => {
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      });

      expect(byTestId("onboarding.agentPreset.codex.status")?.textContent).toBe("Not installed");
      // P6 (#2745) supersedes the P5 text: an event-driven failure has output.
      expect(failedLine("codex")?.textContent).toBe("Install failed.");
      const button = installButton("codex")!;
      expect(button.getAttribute("aria-disabled")).toBe("false");
      expect(button.getAttribute("data-ac-state")).toBe("idle");

      dispose();
    });

    it("install_2736_event_for_another_key_does_not_touch_this_row", async () => {
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();
      await emitFinished({
        key: "mine",
        command: MINE_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      });

      expect(installButton("codex")?.getAttribute("data-ac-state")).toBe("installing");
      expect(installButton("codex")?.textContent).toBe("Installing...");
      expect(failedLine("codex")).toBeNull();

      dispose();
    });

    it("install_2736_retry_after_a_failure_is_allowed", async () => {
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      });
      expect(failedLine("codex")).not.toBeNull();

      installButton("codex")!.click();
      await settle();

      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(2);
      expect(failedLine("codex")).toBeNull();
      expect(installButton("codex")?.getAttribute("data-ac-state")).toBe("installing");

      dispose();
    });

    it("install_2736_installed_row_renders_no_install_row_even_if_a_command_exists", async () => {
      const dispose = await mountReady();

      expect(byTestId("onboarding.agentPreset.claude.status")?.textContent).toBe("Installed");
      expect(installRow("claude")).toBeNull();
      expect(commandText("claude")).toBeNull();

      dispose();
    });

    it("install_2736_listener_is_disposed_on_unmount", async () => {
      const dispose = await mountReady();
      const calls = vi.mocked(CodingAgentsAPI.welcomeStatus).mock.calls.length;

      dispose();
      expect(unlisten).toHaveBeenCalledTimes(1);
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });
      expect(CodingAgentsAPI.welcomeStatus).toHaveBeenCalledTimes(calls);

      // Late resolution: unmount while the listen promise is still pending.
      document.body.innerHTML = "";
      let resolveListen!: (fn: () => void) => void;
      vi.mocked(onCodingAgentInstallFinished).mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveListen = resolve;
          }),
      );
      const lateUnlisten = vi.fn();
      const disposeLate = renderInstall(true);
      await settle();
      disposeLate();
      expect(lateUnlisten).not.toHaveBeenCalled();
      resolveListen(lateUnlisten);
      await settle();
      expect(lateUnlisten).toHaveBeenCalledTimes(1);
    });

    it("install_2736_no_control_is_nested_inside_the_card_button", async () => {
      const dispose = await mountReady();

      const cards = Array.from(
        document.querySelectorAll<HTMLElement>('[data-ac-role="agent-preset"]'),
      );
      expect(cards.length).toBe(5);
      for (const el of cards) {
        expect(el.querySelectorAll("button").length).toBe(0);
      }
      for (const key of ["codex", "mine"]) {
        expect(copyButton(key)!.closest("button")).toBe(copyButton(key));
        expect(installButton(key)!.closest("button")).toBe(installButton(key));
        expect(card(key).contains(copyButton(key))).toBe(false);
      }

      dispose();
    });

    it("install_2736_tab_order_is_card_then_copy_then_install", async () => {
      const dispose = await mountReady();

      const order = focusables();
      for (const key of ["codex", "mine"]) {
        const cardIndex = order.indexOf(card(key));
        expect(cardIndex).toBeGreaterThanOrEqual(0);
        expect(order[cardIndex + 1]).toBe(copyButton(key));
        expect(order[cardIndex + 2]).toBe(installButton(key));
        const nextRow = card(key).parentElement!.nextElementSibling;
        const nextCard = nextRow?.querySelector('[data-ac-role="agent-preset"]');
        expect(nextCard).toBeTruthy();
        expect(order[cardIndex + 3]).toBe(nextCard);
      }

      dispose();
    });

    it("install_2736_copy_and_install_are_keyboard_activatable", async () => {
      const writeText = vi.fn(() => Promise.resolve());
      stubClipboard(writeText);
      const dispose = await mountReady();

      for (const button of [copyButton("mine")!, installButton("mine")!]) {
        expect(button.tagName).toBe("BUTTON");
        expect(button.getAttribute("type")).toBe("button");
        button.focus();
        expect(document.activeElement).toBe(button);
        // A native button turns Enter/Space into this click; jsdom does not
        // synthesise it from a keydown.
        button.click();
        await settle();
      }

      expect(writeText).toHaveBeenCalledWith(MINE_CMD);
      expect(CodingAgentsAPI.install).toHaveBeenCalledWith("mine");

      dispose();
    });

    it("install_2736_both_controls_have_an_accessible_name_naming_the_agent", async () => {
      const dispose = await mountReady();

      expect(copyButton("codex")?.getAttribute("aria-label")).toBe(
        "Copy the install command for Codex",
      );
      expect(installButton("codex")?.getAttribute("aria-label")).toBe("Install Codex");
      expect(copyButton("mine")?.getAttribute("aria-label")).toBe(
        "Copy the install command for My Agent",
      );
      expect(installButton("mine")?.getAttribute("aria-label")).toBe("Install My Agent");
      expect(copyButton("codex")?.getAttribute("title")).toBe("Copy");

      dispose();
    });

    it("install_2736_the_flag_off_row_adds_no_focusable_element_and_no_aria", async () => {
      const dispose = renderInstall(undefined);
      await settle();

      const wrappers = Array.from(document.querySelectorAll<HTMLElement>(".onboarding-card-row"));
      expect(wrappers.length).toBe(5);
      for (const wrapper of wrappers) {
        expect(Array.from(wrapper.attributes).map((attr) => attr.name)).toEqual(["class"]);
        const inner = focusables().filter((el) => wrapper.contains(el));
        expect(inner.length).toBe(1);
        expect(inner[0].getAttribute("data-ac-role")).toBe("agent-preset");
      }
      expect(card("codex").getAttribute("aria-label")).toBe("Select Codex");
      const testIds = Array.from(
        document.querySelectorAll('[data-ac-testid^="onboarding.agentPreset."]'),
      ).map((el) => el.getAttribute("data-ac-testid"));
      expect(testIds).toEqual(cardKeys().map((key) => `onboarding.agentPreset.${key}`));

      dispose();
    });

    it("install_2736_focus_stays_in_the_modal_while_installing", async () => {
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);

      const button = installButton("codex")!;
      button.focus();
      button.click();
      await settle();

      expect(document.activeElement).toBe(installButton("codex"));
      expect(modal().contains(document.activeElement)).toBe(true);
      expect(button.getAttribute("aria-disabled")).toBe("true");
      expect(button.hasAttribute("disabled")).toBe(false);
      button.click();
      await settle();
      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(1);

      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      expect(onCancel).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("install_2736_focus_moves_to_the_card_when_the_install_row_unmounts", async () => {
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockResolvedValueOnce(statusAfterCodexInstall());
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);

      installButton("codex")!.focus();
      installButton("codex")!.click();
      await settle();
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });

      expect(installRow("codex")).toBeNull();
      expect(document.activeElement).toBe(card("codex"));
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      expect(onCancel).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("install_2736_install_is_inert_until_the_listener_promise_resolves", async () => {
      let resolveListen!: (fn: () => void) => void;
      vi.mocked(onCodingAgentInstallFinished).mockImplementationOnce((callback) => {
        finishedHandler = callback;
        return new Promise((resolve) => {
          resolveListen = resolve;
        });
      });
      const dispose = renderInstall(true);
      await settle();

      const button = () => installButton("codex")!;
      expect(button().getAttribute("aria-disabled")).toBe("true");
      expect(button().getAttribute("data-ac-state")).toBe("pending");
      expect(button().textContent).toBe("Install");
      button().click();
      await settle();
      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(0);

      resolveListen(vi.fn());
      await settle();
      expect(button().getAttribute("data-ac-state")).toBe("idle");
      button().click();
      await settle();
      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(1);

      // A fast failure completion must settle the row.
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      });
      expect(button().getAttribute("data-ac-state")).toBe("idle");
      expect(button().getAttribute("aria-disabled")).toBe("false");
      expect(failedLine("codex")?.textContent).toBe("Install failed.");

      dispose();
    });

    it("install_2736_a_rejected_listener_registration_blocks_install_and_shows_the_hint", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      const unhandled = vi.fn();
      // Node test host: an unhandled rejection is reported on the process.
      const host = (globalThis as unknown as { process: NodeEventHost }).process;
      host.on("unhandledRejection", unhandled);
      vi.mocked(onCodingAgentInstallFinished).mockImplementationOnce(() =>
        Promise.reject(new Error("listen failed")),
      );
      const writeText = vi.fn(() => Promise.resolve());
      stubClipboard(writeText);
      const dispose = renderInstall(true);
      await settle();

      expect(modal()).not.toBeNull();
      copyButton("codex")!.click();
      await settle();
      expect(writeText).toHaveBeenCalledWith(CODEX_CMD);

      const button = installButton("codex")!;
      expect(button.getAttribute("aria-disabled")).toBe("true");
      expect(button.getAttribute("data-ac-state")).toBe("blocked");
      button.click();
      await settle();
      expect(CodingAgentsAPI.install).toHaveBeenCalledTimes(0);
      expect(failedLine("codex")?.textContent).toBe("Install is unavailable; see the app log.");
      expect(consoleError).toHaveBeenCalled();
      expect(unhandled).not.toHaveBeenCalled();

      host.off("unhandledRejection", unhandled);
      consoleError.mockRestore();
      dispose();
    });

    function deferredStatus() {
      let resolve!: (rows: CodingAgentWelcomeStatus[]) => void;
      let reject!: (reason: unknown) => void;
      const promise = new Promise<CodingAgentWelcomeStatus[]>((res, rej) => {
        resolve = res;
        reject = rej;
      });
      return { promise, resolve, reject };
    }

    const bothInstalled = (): CodingAgentWelcomeStatus[] => [
      row("codex", true, CODEX_CMD, "high"),
      row("claude", true, "npm install -g claude", "medium"),
      row("pi", false, null, "low"),
      row("mine", true, MINE_CMD),
    ];

    it("install_2736_another_rows_completion_keeps_focus_in_the_modal", async () => {
      const readA = deferredStatus();
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockReturnValueOnce(readA.promise)
        .mockResolvedValueOnce(bothInstalled());
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);

      installButton("codex")!.click();
      installButton("mine")!.click();
      await settle();
      installButton("codex")!.focus();

      // Codex completes; its status read stays pending.
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });
      expect(document.activeElement).toBe(installButton("codex"));
      // Mine completes; its read removes BOTH install rows.
      await emitFinished({
        key: "mine",
        command: MINE_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });

      expect(installRow("codex")).toBeNull();
      expect(document.activeElement).toBe(card("codex"));
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      expect(onCancel).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("install_2736_an_older_status_response_does_not_overwrite_a_newer_one", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      const codexOnly = (): CodingAgentWelcomeStatus[] => [
        row("codex", true, CODEX_CMD, "high"),
        row("claude", true, "npm install -g claude", "medium"),
        row("pi", false, null, "low"),
        row("mine", false, MINE_CMD),
      ];
      const mineIsLatest = () => {
        expect(byTestId("onboarding.agentPreset.mine.status")?.textContent).toBe("Installed");
        expect(installRow("mine")).toBeNull();
        const keys = cardKeys();
        expect(keys.indexOf("mine")).toBeLessThan(keys.indexOf("pi"));
      };

      for (const lateA of ["resolve", "reject"] as const) {
        const readA = deferredStatus();
        const readB = deferredStatus();
        vi.mocked(CodingAgentsAPI.welcomeStatus)
          .mockResolvedValueOnce(installRows())
          .mockReturnValueOnce(readA.promise)
          .mockReturnValueOnce(readB.promise);
        const dispose = await mountReady();

        installButton("codex")!.click();
        installButton("mine")!.click();
        await settle();
        await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      }); // read A
        await emitFinished({
        key: "mine",
        command: MINE_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      }); // read B

        readB.resolve(bothInstalled());
        await settle();
        mineIsLatest();

        if (lateA === "resolve") readA.resolve(codexOnly());
        else readA.reject("late failure");
        await settle();
        mineIsLatest();
        expect(consoleError).not.toHaveBeenCalled();

        dispose();
        document.body.innerHTML = "";
      }

      consoleError.mockRestore();
    });

    it("install_2736_a_failed_status_read_keeps_focus_in_the_modal", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockRejectedValueOnce("status failure");
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);

      installButton("codex")!.focus();
      installButton("codex")!.click();
      await settle();
      // The current read fails: the empty status removes every install row.
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      });

      expect(consoleError).toHaveBeenCalledTimes(1);
      expect(installRow("codex")).toBeNull();
      expect(document.activeElement).toBe(card("codex"));
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      expect(onCancel).toHaveBeenCalledTimes(1);

      consoleError.mockRestore();
      dispose();
    });

    // #2745 (P6) - install failure output disclosure. Continues the P5 numbering at 32.
    const outputToggle = (key: string) =>
      byTestId<HTMLButtonElement>(`onboarding.agentPreset.${key}.outputToggle`);
    const outputBox = (key: string) => byTestId(`onboarding.agentPreset.${key}.output`);
    const FAIL_STDOUT = "added 0 packages\n";
    const FAIL_STDERR = "npm ERR! code E404\r\n";

    async function failInstall(
      key: string,
      command: string,
      extra: Partial<CodingAgentInstallFinished> = {},
    ): Promise<void> {
      installButton(key)!.click();
      await settle();
      await emitFinished({
        key,
        command,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: FAIL_STDOUT,
        stderr: FAIL_STDERR,
        ...extra,
      });
    }

    it("install_2736_format_install_output_is_byte_exact", () => {
      const base: CodingAgentInstallFinished = {
        key: "codex",
        command: "npm i -g x",
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "",
      };
      const marker = "[... output truncated to the last 8192 bytes ...]\n";
      const cases: Array<[Partial<CodingAgentInstallFinished>, string]> = [
        [
          { stdout: "out", stderr: "err", exitCode: 3, detail: "exit code 3" },
          "$ npm i -g x\nout\nerr\nexit code: 3 (exit code 3)",
        ],
        [
          { stderr: "err", exitCode: 2, detail: "exit code 2" },
          "$ npm i -g x\nerr\nexit code: 2 (exit code 2)",
        ],
        [
          { exitCode: null, detail: "timed out after 300s (killed)" },
          "$ npm i -g x\nexit code: n/a (timed out after 300s (killed))",
        ],
        [
          { stdout: "o", exitCode: null, detail: "spawn failed" },
          "$ npm i -g x\no\nexit code: n/a (spawn failed)",
        ],
        [{ stdout: `${marker}  tail` }, `$ npm i -g x\n${marker}  tail\nexit code: 1 (exit code 1)`],
        [{ stdout: "a\n", stderr: "b\r\n" }, "$ npm i -g x\na\nb\nexit code: 1 (exit code 1)"],
        [{ stdout: "a\n\n" }, "$ npm i -g x\na\n\nexit code: 1 (exit code 1)"],
      ];
      for (const [fields, expected] of cases) {
        const text = formatInstallOutput({ ...base, ...fields });
        expect(text).toBe(expected);
        expect(text).not.toContain("\r");
      }
    });

    it("install_2736_ok_false_event_shows_a_collapsed_disclosure", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);

      expect(failedLine("codex")?.textContent).toBe("Install failed.");
      expect(outputToggle("codex")?.getAttribute("aria-expanded")).toBe("false");
      expect(outputToggle("codex")?.textContent).toBe("Show output");
      expect(outputToggle("codex")?.getAttribute("aria-label")).toBe(
        "Show the install output for Codex",
      );
      expect(outputBox("codex")).toBeNull();

      dispose();
    });

    it("install_2736_show_output_expands_and_hide_collapses", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);

      const toggle = outputToggle("codex")!;
      toggle.focus();
      toggle.click();
      await settle();
      expect(toggle.getAttribute("aria-expanded")).toBe("true");
      expect(toggle.textContent).toBe("Hide output");
      const box = outputBox("codex")!;
      expect(box.tagName).toBe("PRE");
      expect(box.textContent).toBe(
        formatInstallOutput({
          key: "codex",
          command: CODEX_CMD,
          ok: false,
          detail: "exit code 1",
          exitCode: 1,
          stdout: FAIL_STDOUT,
          stderr: FAIL_STDERR,
        }),
      );
      expect(box.getAttribute("tabindex")).toBe("0");
      // jsdom cannot measure the scroll; the declarations pin the row width.
      // The variable base keeps the real file: URL (see sidebar-compact.test.ts).
      const moduleUrl = import.meta.url;
      const css = readFileSync(new URL("../styles/sidebar.css", moduleUrl), "utf8");
      const body = soleRuleBody(
        scanRules(css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "))),
        ".onboarding-install-output",
      );
      expect(declValue(body, "overflow")).toBe("auto");
      expect(declValue(body, "white-space")).toBe("pre-wrap");

      toggle.click();
      await settle();
      expect(outputBox("codex")).toBeNull();
      expect(outputToggle("codex")).toBe(toggle);
      expect(document.activeElement).toBe(toggle);
      expect(toggle.getAttribute("aria-expanded")).toBe("false");

      dispose();
    });

    it("install_2736_a_refused_invoke_shows_no_output_toggle", async () => {
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      vi.mocked(CodingAgentsAPI.install).mockRejectedValue("already running");
      const dispose = await mountReady();

      installButton("codex")!.click();
      await settle();

      expect(failedLine("codex")?.textContent).toBe("Install failed; see the app log.");
      expect(outputToggle("codex")).toBeNull();
      expect(outputBox("codex")).toBeNull();

      consoleError.mockRestore();
      dispose();
    });

    it("install_2736_output_box_renders_process_text_as_text", async () => {
      const stderr = '<img src=x onerror="alert(1)"> &amp; done';
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD, { stdout: "", stderr });

      outputToggle("codex")!.click();
      await settle();
      const box = outputBox("codex")!;
      expect(box.textContent).toBe(`$ ${CODEX_CMD}\n${stderr}\nexit code: 1 (exit code 1)`);
      expect(box.querySelector("img")).toBeNull();
      expect(box.children.length).toBe(0);

      dispose();
    });

    it("install_2736_expanded_output_stays_inside_the_focus_trap", async () => {
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);
      await failInstall("codex", CODEX_CMD);

      outputToggle("codex")!.click();
      await settle();
      const box = outputBox("codex")!;
      box.focus();
      expect(document.activeElement).toBe(box);
      box.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(onCancel).toHaveBeenCalledTimes(1);
      expect(focusables()).toContain(box);
      expect(focusables()).toContain(outputToggle("codex"));

      dispose();
    });

    it("install_2736_expanded_tab_order_is_card_copy_install_toggle_output", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();

      const order = focusables();
      const cardIndex = order.indexOf(card("codex"));
      expect(cardIndex).toBeGreaterThanOrEqual(0);
      expect(order.slice(cardIndex, cardIndex + 5)).toEqual([
        card("codex"),
        copyButton("codex"),
        installButton("codex"),
        outputToggle("codex"),
        outputBox("codex"),
      ]);
      const nextRow = card("codex").parentElement!.nextElementSibling;
      const nextCard = nextRow?.querySelector('[data-ac-role="agent-preset"]');
      expect(nextCard).toBeTruthy();
      expect(order[cardIndex + 5]).toBe(nextCard);

      dispose();
    });

    it("install_2736_retry_clears_the_stored_output_and_the_expanded_flag", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();
      expect(outputBox("codex")).not.toBeNull();

      installButton("codex")!.click();
      await settle();
      expect(installButton("codex")?.getAttribute("data-ac-state")).toBe("installing");
      expect(failedLine("codex")).toBeNull();
      expect(outputToggle("codex")).toBeNull();
      expect(outputBox("codex")).toBeNull();

      // A repeat failure opens collapsed.
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: "",
        stderr: "again",
      });
      expect(outputToggle("codex")?.getAttribute("aria-expanded")).toBe("false");
      expect(outputBox("codex")).toBeNull();

      dispose();
    });

    it("install_2736_ok_true_clears_a_previously_failed_rows_disclosure", async () => {
      vi.mocked(CodingAgentsAPI.welcomeStatus)
        .mockResolvedValueOnce(installRows())
        .mockResolvedValueOnce(installRows())
        .mockResolvedValueOnce(statusAfterCodexInstall());
      const onCancel = vi.fn();
      const dispose = await mountReady(onCancel);
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();
      outputBox("codex")!.focus();
      expect(document.activeElement).toBe(outputBox("codex"));

      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: true,
        detail: "ok",
        exitCode: 0,
        stdout: "",
        stderr: "",
      });

      expect(failedLine("codex")).toBeNull();
      expect(outputToggle("codex")).toBeNull();
      expect(installRow("codex")).toBeNull();
      expect(outputBox("codex")).toBeNull();
      expect(document.activeElement).toBe(card("codex"));
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
      expect(onCancel).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("install_2736_a_second_rows_failure_does_not_expand_this_one", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      await failInstall("mine", MINE_CMD);

      outputToggle("codex")!.click();
      await settle();
      expect(outputToggle("codex")?.getAttribute("aria-expanded")).toBe("true");
      expect(outputToggle("mine")?.getAttribute("aria-expanded")).toBe("false");
      expect(outputBox("mine")).toBeNull();

      outputToggle("mine")!.click();
      await settle();
      expect(outputBox("codex")?.textContent?.startsWith(`$ ${CODEX_CMD}\n`)).toBe(true);
      expect(outputBox("mine")?.textContent?.startsWith(`$ ${MINE_CMD}\n`)).toBe(true);

      dispose();
    });

    // A catalog refresh that drops (false) or restores (true) codex.
    async function refreshCatalog(withCodex: boolean): Promise<void> {
      const catalog = installCatalog().filter((def) => withCodex || def.key !== "codex");
      vi.mocked(CodingAgentsAPI.getCatalogReport).mockResolvedValue(report({ catalog }));
      await codingAgentsStore.refresh();
      await settle();
      expect(card("codex") !== null).toBe(withCodex);
    }

    function expectCleanCodexRow(): void {
      expect(installButton("codex")).not.toBeNull();
      expect(failedLine("codex")).toBeNull();
      expect(outputToggle("codex")).toBeNull();
      expect(outputBox("codex")).toBeNull();
      expect(installButton("codex")?.getAttribute("aria-disabled")).toBe("false");
      expect(installButton("codex")?.getAttribute("data-ac-state")).toBe("idle");
    }

    it("install_2736_a_removed_and_restored_agent_shows_no_failure_state", async () => {
      const dispose = await mountReady();
      installButton("codex")!.click();
      await settle();

      await refreshCatalog(false);
      await emitFinished({
        key: "codex",
        command: CODEX_CMD,
        ok: false,
        detail: "exit code 1",
        exitCode: 1,
        stdout: FAIL_STDOUT,
        stderr: FAIL_STDERR,
      });
      await refreshCatalog(true);

      expectCleanCodexRow();

      dispose();
    });

    it("install_2736_catalog_refresh_prunes_a_removed_keys_failure_state", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();
      expect(outputBox("codex")).not.toBeNull();

      await refreshCatalog(false);
      await refreshCatalog(true);

      expectCleanCodexRow();

      dispose();
    });

    it("install_2736_a_refresh_that_keeps_the_agent_keeps_its_failure", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();
      const text = outputBox("codex")!.textContent;

      await refreshCatalog(true);

      expect(failedLine("codex")?.textContent).toBe("Install failed.");
      expect(outputToggle("codex")?.getAttribute("aria-expanded")).toBe("true");
      expect(outputBox("codex")?.textContent).toBe(text);

      dispose();
    });

    it("install_2736_the_mid_refresh_window_does_not_lose_the_state", async () => {
      const dispose = await mountReady();
      await failInstall("codex", CODEX_CMD);
      outputToggle("codex")!.click();
      await settle();
      const text = outputBox("codex")!.textContent;

      let resolveReport!: (value: CatalogReport) => void;
      const pendingReport = new Promise<CatalogReport>((resolve) => {
        resolveReport = resolve;
      });
      vi.mocked(CodingAgentsAPI.getCatalogReport).mockReturnValueOnce(pendingReport);
      const refreshing = codingAgentsStore.refresh();
      await settle();

      // The window was really entered: loading, and the row unmounted.
      expect(byTestId("onboarding.catalog.loading")).not.toBeNull();
      expect(card("codex")).toBeNull();

      resolveReport(report({ catalog: installCatalog() }));
      await refreshing;
      await settle();

      expect(card("codex")).not.toBeNull();
      expect(failedLine("codex")?.textContent).toBe("Install failed.");
      expect(outputToggle("codex")?.getAttribute("aria-expanded")).toBe("true");
      expect(outputBox("codex")?.textContent).toBe(text);

      dispose();
    });
  });
});
