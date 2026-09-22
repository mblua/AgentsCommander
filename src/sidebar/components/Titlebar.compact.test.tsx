// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSettings } from "../../shared/types";
import Titlebar from "./Titlebar";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  installBrowserDomStubs,
  registerCompactHostForTests,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import { setSidebarCompactMode, sidebarCompact } from "../../shared/sidebar-compact";
import { MAIN_SIDEBAR_MIN_WIDTH } from "../../shared/sidebar-layout";

// #2236 phase 6 — width presets are inert while compact. Two independent
// layers are proved apart: the early return is reached directly (test 2a) and
// the `disabled` attribute is asserted on its own (test 2b), so deleting
// either one turns exactly one test red. A click on a `disabled` button never
// reaches the handler, which is why 2a removes the attribute first.

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("../../shared/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../shared/ipc")>();
  return {
    ...actual,
    SettingsAPI: {
      ...actual.SettingsAPI,
      get: mocks.getSettings,
      update: mocks.updateSettings,
    },
  };
});

function mountTitlebar() {
  const fake = new FakeTransport();
  fake.resolve("screenshot_get_hotkey_status", {
    configured: "Ctrl+Q",
    registered: true,
    error: null,
  });
  return renderWithFakeTransport(() => <Titlebar />, fake);
}

function presetButtons(root: ParentNode): HTMLButtonElement[] {
  return Array.from(root.querySelectorAll<HTMLButtonElement>(".layout-option"));
}

function openLayoutMenu(root: ParentNode): void {
  const toggle = root.querySelector<HTMLElement>('[data-ac-testid="titlebar.layout.button"]');
  if (!toggle) throw new Error("missing titlebar.layout.button");
  click(toggle);
}

function captureEvents<T>(name: string): { received: T[]; stop: () => void } {
  const received: T[] = [];
  const listener = (event: Event) => received.push((event as CustomEvent<T>).detail);
  window.addEventListener(name, listener);
  return {
    received,
    stop: () => window.removeEventListener(name, listener),
  };
}

async function flushMicrotasks(): Promise<void> {
  for (let index = 0; index < 8; index += 1) await Promise.resolve();
}

describe("Titlebar width presets vs compact (#2236)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    mocks.getSettings.mockReset().mockResolvedValue(baseSettings());
    mocks.updateSettings.mockReset().mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    vi.clearAllMocks();
  });

  it("applies a width preset with both effects while expanded", async () => {
    const widthChanges = captureEvents<{ width: number }>("main-sidebar-width-change");
    const rendered = mountTitlebar();
    try {
      openLayoutMenu(rendered.root);
      const preset = presetButtons(rendered.root)[0];
      expect(preset).toBeDefined();
      click(preset);

      expect(widthChanges.received).toEqual([{ width: MAIN_SIDEBAR_MIN_WIDTH }]);
      await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
      expect((mocks.updateSettings.mock.calls[0][0] as AppSettings).mainSidebarWidth).toBe(
        MAIN_SIDEBAR_MIN_WIDTH,
      );
    } finally {
      widthChanges.stop();
      rendered.cleanup();
    }
  });

  it("suppresses both preset effects while compact with the disabled attribute removed", async () => {
    const widthChanges = captureEvents<{ width: number }>("main-sidebar-width-change");
    const rendered = mountTitlebar();
    try {
      registerCompactHostForTests();
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);

      openLayoutMenu(rendered.root);
      const preset = presetButtons(rendered.root)[0];
      expect(preset).toBeDefined();
      // Reach the early return directly: the attribute can no longer decide.
      preset.disabled = false;
      preset.removeAttribute("disabled");
      expect(preset.disabled).toBe(false);
      click(preset);
      await flushMicrotasks();

      expect(widthChanges.received).toHaveLength(0);
      expect(mocks.updateSettings).not.toHaveBeenCalled();
    } finally {
      widthChanges.stop();
      rendered.cleanup();
    }
  });

  it("marks the preset buttons disabled while compact and enabled expanded", async () => {
    const rendered = mountTitlebar();
    try {
      openLayoutMenu(rendered.root);
      expect(presetButtons(rendered.root).length).toBeGreaterThan(0);
      expect(presetButtons(rendered.root).some((button) => button.disabled)).toBe(false);

      registerCompactHostForTests();
      setSidebarCompactMode(true);
      expect(presetButtons(rendered.root).every((button) => button.disabled)).toBe(true);

      setSidebarCompactMode(false);
      expect(presetButtons(rendered.root).some((button) => button.disabled)).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("keeps the side preset working while compact", async () => {
    const sideChanges = captureEvents<{ side: string }>("main-sidebar-side-change");
    const rendered = mountTitlebar();
    try {
      registerCompactHostForTests();
      setSidebarCompactMode(true);

      openLayoutMenu(rendered.root);
      const sidePreset = rendered.root.querySelector<HTMLButtonElement>(".layout-segment");
      expect(sidePreset).not.toBeNull();
      click(sidePreset!);

      expect(sideChanges.received).toEqual([{ side: "left" }]);
      await waitFor(() => expect(mocks.updateSettings).toHaveBeenCalledTimes(1));
      expect((mocks.updateSettings.mock.calls[0][0] as AppSettings).mainSidebarSide).toBe("left");
    } finally {
      sideChanges.stop();
      rendered.cleanup();
    }
  });
});
