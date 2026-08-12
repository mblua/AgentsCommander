// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal, { isPlausibleCompleteExecutablePath } from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";

// #1313 — the Default Shell field warns (informational red banner) while its
// value is not a plausible complete executable path, i.e. it contains no
// directory separator. The check is a pure shape heuristic: no filesystem
// access, no platform sniffing, and it never blocks saving.

describe("isPlausibleCompleteExecutablePath (#1313)", () => {
  it.each([
    // Bare names: no directory separator -> not a complete path.
    ["powershell.exe", false],
    ["pwsh", false],
    ["Powershell.Exe", false],
    ["C:foo.exe", false],
    // Empty / whitespace-only: claims no executable -> no warning.
    ["", true],
    ["   ", true],
    // Anything containing a separator is plausible and exempt.
    ["C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", true],
    ["\\\\server\\share\\pwsh.exe", true],
    ["%SystemRoot%\\System32\\cmd.exe", true],
    ["/bin/bash", true],
    ["~/bin/zsh", true],
  ])("returns %j for %j", (value, expected) => {
    expect(isPlausibleCompleteExecutablePath(value)).toBe(expected);
  });
});

describe("SettingsModal Default Shell warning (#1313)", () => {
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

  function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
    return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  }

  function renderShell() {
    const fake = new FakeTransport();
    fake.resolve("get_settings", baseSettings({ defaultShell: "powershell.exe" }));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    return renderWithFakeTransport(() => <SettingsModal section="general" onClose={() => {}} />, fake);
  }

  it("warns for a bare name while typing, never blocks save, and clears for a complete path", async () => {
    const r = renderShell();
    try {
      await waitFor(() => expect(byTestId(r.root, "settings.general.defaultShell")).toBeTruthy());

      const input = byTestId<HTMLInputElement>(r.root, "settings.general.defaultShell")!;
      expect(input.closest("label.settings-field")?.querySelector(".settings-label")?.textContent).toBe(
        "Default Shell (Complete path)"
      );

      // Bare "powershell.exe" (factory default on Windows): banner visible.
      expect(byTestId(r.root, "settings.general.defaultShell.warning")).toBeTruthy();

      // Informational only: save stays enabled.
      const save = byTestId<HTMLButtonElement>(r.root, "settings.save")!;
      expect(save.disabled).toBe(false);

      // Typing a complete path clears the banner live, without saving.
      input.value = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await waitFor(() => expect(byTestId(r.root, "settings.general.defaultShell.warning")).toBeNull());

      // Typing back to the bare name brings the banner back, live.
      const input2 = byTestId<HTMLInputElement>(r.root, "settings.general.defaultShell")!;
      input2.value = "powershell.exe";
      input2.dispatchEvent(new Event("input", { bubbles: true }));
      await waitFor(() => expect(byTestId(r.root, "settings.general.defaultShell.warning")).toBeTruthy());
    } finally {
      r.cleanup();
    }
  });
});
