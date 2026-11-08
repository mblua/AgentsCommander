// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// #4049 - both platforms are explicit: the seam flips the platform per test.
const platform = vi.hoisted(() => ({ tauri: true }));
vi.mock("../../shared/platform", () => ({
  get isTauri() { return platform.tauri; },
  get isBrowser() { return !platform.tauri; },
  isWindows: false,
}));

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
import {
  __disposeSettingsSubscriptionsForTests,
  settingsStore,
} from "../../shared/stores/settings";
import { setSidebarCompactMode } from "../../shared/sidebar-compact";
import { centralViewStore } from "../../main/stores/centralView";

const CTX = '[data-ac-testid="actionBar.contextHistory"]';
const HOME = '[data-ac-testid="actionBar.home"]';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function settings(overrides: Partial<AppSettings>, omitEnabled = false): AppSettings {
  const value = baseSettings(overrides);
  if (omitEnabled) delete value.contextHistoryEnabled;
  return value;
}

function setup(value: AppSettings): FakeTransport {
  const fake = new FakeTransport();
  fake.resolve("get_settings", value);
  fake.resolve("get_resource_snapshot", null);
  return fake;
}

describe("ActionBar CTX visibility follows contextHistoryEnabled (#4049)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    platform.tauri = true;
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    // The settings signal is module state outside resetUiStoresForTests.
    __disposeSettingsSubscriptionsForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    // The settings signal is module state outside resetUiStoresForTests.
    __disposeSettingsSubscriptionsForTests();
    vi.restoreAllMocks();
  });

  async function render(fake: FakeTransport) {
    const rendered = renderWithFakeTransport(() => <ActionBar />, fake);
    await waitFor(() => expect(rendered.root.querySelector(HOME)).toBeTruthy());
    const ctx = () => rendered.root.querySelector<HTMLButtonElement>(CTX);
    return { rendered, ctx };
  }

  for (const [name, value] of [
    ["missing", settings({}, true)],
    ["false", settings({ contextHistoryEnabled: false })],
  ] as const) {
    it(`hides CTX when the preference is ${name}`, async () => {
      const m = await render(setup(value));
      try {
        await settingsStore.load();
        expect(settingsStore.current).toBeTruthy();
        expect(m.ctx()).toBeNull();
      } finally { m.rendered.cleanup(); }
    });
  }

  it("shows CTX when enabled and the click uses the existing central view", async () => {
    const m = await render(setup(settings({ contextHistoryEnabled: true })));
    try {
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeTruthy());
      expect(centralViewStore.isContextHistory).toBe(false);
      m.ctx()!.click();
      expect(centralViewStore.isContextHistory).toBe(true);
      expect(m.ctx()!.getAttribute("data-ac-state")).toBe("visible");
      m.ctx()!.click();
      expect(centralViewStore.isContextHistory).toBe(false);
    } finally { m.rendered.cleanup(); }
  });

  it("toggles live off and on and keeps an open pane when hidden", async () => {
    const fake = setup(settings({ contextHistoryEnabled: true }));
    const m = await render(fake);
    try {
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeTruthy());
      m.ctx()!.click();
      expect(centralViewStore.isContextHistory).toBe(true);
      fake.resolve("get_settings", settings({ contextHistoryEnabled: false }));
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeNull());
      expect(centralViewStore.isContextHistory).toBe(true);
      fake.resolve("get_settings", settings({ contextHistoryEnabled: true }));
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeTruthy());
    } finally { m.rendered.cleanup(); }
  });

  it("hides CTX while compact", async () => {
    const m = await render(setup(settings({ contextHistoryEnabled: true })));
    try {
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeTruthy());
      registerCompactHostForTests();
      setSidebarCompactMode(true);
      await waitFor(() => expect(m.ctx()).toBeNull());
    } finally { m.rendered.cleanup(); }
  });

  it("the browser hides CTX even when enabled", async () => {
    platform.tauri = false;
    const m = await render(setup(settings({ contextHistoryEnabled: true })));
    try {
      await settingsStore.load();
      expect(settingsStore.current?.contextHistoryEnabled).toBe(true);
      expect(m.ctx()).toBeNull();
    } finally { m.rendered.cleanup(); }
  });

  it("keeps the last accepted value across a pending and a rejected refresh", async () => {
    const fake = setup(settings({ contextHistoryEnabled: true }));
    const m = await render(fake);
    try {
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeTruthy());
      const pending = deferred<AppSettings>();
      fake.onInvoke("get_settings", () => pending.promise);
      const refresh = settingsStore.load();
      expect(m.ctx()).toBeTruthy();
      pending.reject(new Error("settings read failed"));
      await expect(refresh).rejects.toThrow();
      expect(m.ctx()).toBeTruthy();
      fake.resolve("get_settings", settings({ contextHistoryEnabled: false }));
      await settingsStore.load();
      await waitFor(() => expect(m.ctx()).toBeNull());
    } finally { m.rendered.cleanup(); }
  });

  it("hides CTX while the first request is pending and after it is rejected", async () => {
    const fake = setup(settings({ contextHistoryEnabled: true }));
    const pending = deferred<AppSettings>();
    fake.onInvoke("get_settings", () => pending.promise);
    const m = await render(fake);
    try {
      const first = settingsStore.load();
      expect(m.ctx()).toBeNull();
      pending.reject(new Error("settings read failed"));
      await expect(first).rejects.toThrow();
      expect(settingsStore.current).toBeNull();
      expect(m.ctx()).toBeNull();
    } finally { m.rendered.cleanup(); }
  });
});
