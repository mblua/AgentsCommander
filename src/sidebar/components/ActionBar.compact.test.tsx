// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../../shared/types";
import ActionBar from "./ActionBar";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  installBrowserDomStubs,
  registerCompactHostForTests,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { settingsStore } from "../../shared/stores/settings";
import { setSidebarCompactMode, sidebarCompact } from "../../shared/sidebar-compact";

// #2236 phase 6 — compact composition. D11 makes a mode change a no-op without
// a registered host, so every compact arm installs one through the phase-2
// harness first. No prop is passed to ActionBar: the component reads the
// module-level signal directly, and a test that passed a prop would not
// compile against the unchanged `Component` signature.

const COMPACT_TEST_IDS = ["actionBar.theme", "actionBar.settings"] as const;

const EXPANDED_ONLY_TEST_IDS = [
  "actionBar.newOpen",
  "actionBar.specBoard",
  "actionBar.home",
  "actionBar.sortCoordinators",
  "actionBar.sounds",
  "actionBar.categories",
  "actionBar.pinSelectedWorkgroup",
  "actionBar.guide",
  "actionBar.resourceMonitor",
] as const;

function setup(overrides: Partial<AppSettings> = {}): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", baseSettings(overrides));
  fake.resolve("get_resource_snapshot", null);
  return fake;
}

function gearButtons(root: ParentNode): HTMLButtonElement[] {
  return Array.from(root.querySelectorAll<HTMLButtonElement>(".toolbar-gear-btn"));
}

describe("ActionBar compact composition (#2236)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.restoreAllMocks();
  });

  it("renders exactly the theme and settings controls while compact", async () => {
    // The flag is on for this arm on purpose: "specBoard included whatever the
    // flag says" is only proved if the hidden control was present expanded.
    const fake = setup({ specBoardEnabled: true });
    const rendered = renderWithFakeTransport(() => <ActionBar />, fake);
    try {
      await settingsStore.load();
      await waitFor(() => expect(gearButtons(rendered.root)).toHaveLength(10));

      registerCompactHostForTests();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);

      await waitFor(() => expect(gearButtons(rendered.root)).toHaveLength(2));
      expect(
        gearButtons(rendered.root).map((button) => button.getAttribute("data-ac-testid")),
      ).toEqual([...COMPACT_TEST_IDS]);
      for (const testId of EXPANDED_ONLY_TEST_IDS) {
        expect(rendered.root.querySelector(`[data-ac-testid="${testId}"]`)).toBeNull();
      }
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the nine shipped controls expanded when specBoardEnabled is falsy", async () => {
    const fake = setup({ specBoardEnabled: false });
    const rendered = renderWithFakeTransport(() => <ActionBar />, fake);
    try {
      await settingsStore.load();
      await waitFor(() => expect(gearButtons(rendered.root)).toHaveLength(9));
      expect(rendered.root.querySelector('[data-ac-testid="actionBar.specBoard"]')).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("renders specBoard as the tenth control expanded when specBoardEnabled is true", async () => {
    const fake = setup({ specBoardEnabled: true });
    const rendered = renderWithFakeTransport(() => <ActionBar />, fake);
    try {
      await settingsStore.load();
      await waitFor(() => expect(gearButtons(rendered.root)).toHaveLength(10));
      expect(rendered.root.querySelector('[data-ac-testid="actionBar.specBoard"]')).not.toBeNull();
    } finally {
      rendered.cleanup();
    }
  });

  it("resizes neither surviving button and adds no inline sizing while compact", async () => {
    const fake = setup();
    const rendered = renderWithFakeTransport(() => <ActionBar />, fake);
    try {
      registerCompactHostForTests();
      setSidebarCompactMode(true);
      await waitFor(() => expect(gearButtons(rendered.root)).toHaveLength(2));

      for (const testId of COMPACT_TEST_IDS) {
        const button = rendered.root.querySelector<HTMLButtonElement>(
          `[data-ac-testid="${testId}"]`,
        );
        expect(button).not.toBeNull();
        expect(button!.classList.contains("toolbar-gear-btn")).toBe(true);
        expect(button!.className.trim()).toBe("toolbar-gear-btn");
        expect(button!.getAttribute("style")).toBeNull();
      }
    } finally {
      rendered.cleanup();
    }
  });
});
