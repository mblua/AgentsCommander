// @vitest-environment jsdom
import { render } from "solid-js/web";
import type { Component } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ScreenshotHotkeyStatus } from "../../shared/types";

const mocks = vi.hoisted(() => ({
  getHotkeyStatus: vi.fn(),
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(),
    isMaximized: vi.fn(() => Promise.resolve(false)),
    maximize: vi.fn(),
    unmaximize: vi.fn(),
    close: vi.fn(),
  }),
}));

vi.mock("../../shared/ipc", () => ({
  getIsolatedPackageTitlebarIdentity: async () => ({ mode: "normal" as const }),
  InvalidIsolatedPackageTitlebarIdentityResponseError: class extends Error {},
  ScreenshotAPI: {
    getHotkeyStatus: mocks.getHotkeyStatus,
  },
  SettingsAPI: {
    get: mocks.getSettings,
    update: mocks.updateSettings,
  },
}));

vi.mock("./WebServerMenu", () => ({
  default: () => <span data-testid="webserver-menu" />,
}));

vi.mock("./ZoomStepper", () => ({
  default: () => <span data-testid="zoom-stepper" />,
}));

interface MountOptions {
  native?: boolean;
  status?: unknown;
  rejectStatus?: boolean;
}

interface MountedTitlebar {
  root: HTMLDivElement;
  cleanup: () => void;
}

const cleanups: Array<() => void> = [];
const tauriGlobalKeys = ["__TAURI_INTERNALS__", "__TAURI__"] as const;

const screenshotStatus = (
  overrides: Partial<ScreenshotHotkeyStatus> = {},
): ScreenshotHotkeyStatus => ({
  configured: "Ctrl+1",
  registered: true,
  error: null,
  ...overrides,
});

const setNativeRuntime = (native: boolean) => {
  for (const key of tauriGlobalKeys) {
    if (native) {
      Reflect.set(window, key, {});
    } else {
      Reflect.deleteProperty(window, key);
    }
  }
};

const flushAsyncWork = async () => {
  await vi.dynamicImportSettled();
  await Promise.resolve();
};

const mountTitlebar = async ({
  native = true,
  status = screenshotStatus(),
  rejectStatus = false,
}: MountOptions = {}): Promise<MountedTitlebar> => {
  setNativeRuntime(native);
  mocks.getSettings.mockResolvedValue({ mainSidebarSide: "right" });
  mocks.invoke.mockResolvedValue("");
  if (rejectStatus) {
    mocks.getHotkeyStatus.mockRejectedValue(new Error("status unavailable"));
  } else {
    mocks.getHotkeyStatus.mockResolvedValue(status);
  }

  vi.resetModules();
  const { default: Titlebar } = await import("./Titlebar");
  const root = document.createElement("div");
  document.body.append(root);
  const cleanup = render(() => <Titlebar />, root);
  cleanups.push(() => {
    cleanup();
    root.remove();
  });

  await flushAsyncWork();
  return { root, cleanup };
};

const getChip = (root: Element): HTMLElement | null =>
  root.querySelector<HTMLElement>("[data-ac-testid='screenshot-hotkey-status']");

describe("Titlebar screenshot hotkey status", () => {
  beforeEach(() => {
    mocks.getHotkeyStatus.mockReset();
    mocks.getSettings.mockReset();
    mocks.updateSettings.mockReset();
    mocks.invoke.mockReset();
  });

  afterEach(() => {
    while (cleanups.length > 0) cleanups.pop()?.();
    setNativeRuntime(false);
    vi.clearAllMocks();
  });

  it("renders an active native status before WebServerMenu and ZoomStepper", async () => {
    const { root } = await mountTitlebar();
    const controls = root.querySelector<HTMLElement>(".titlebar-controls");
    const chip = getChip(root);
    const webServerMenu = root.querySelector<HTMLElement>("[data-testid='webserver-menu']");
    const zoomStepper = root.querySelector<HTMLElement>("[data-testid='zoom-stepper']");

    expect(controls).not.toBeNull();
    expect(chip).not.toBeNull();
    expect(webServerMenu).not.toBeNull();
    expect(zoomStepper).not.toBeNull();
    expect(chip?.querySelector(".screenshot-hotkey-status-text")?.textContent).toBe("Ctrl + 1");

    expect(chip?.parentElement).toBe(controls);
    expect(webServerMenu?.parentElement).toBe(controls);
    expect(zoomStepper?.parentElement).toBe(controls);
    expect(webServerMenu?.previousElementSibling).toBe(chip);
    expect(zoomStepper?.previousElementSibling).toBe(webServerMenu);
  });

  it("formats multipart combinations without changing token order or spelling", async () => {
    const { root } = await mountTitlebar({
      status: screenshotStatus({ configured: "Ctrl+Shift+F9" }),
    });

    expect(getChip(root)?.querySelector(".screenshot-hotkey-status-text")?.textContent)
      .toBe("Ctrl + Shift + F9");
  });

  it("is accessible but exposes no activation semantics", async () => {
    const { root } = await mountTitlebar();
    const chip = getChip(root);

    expect(chip).not.toBeNull();
    expect(chip?.tagName).toBe("SPAN");
    expect(chip?.getAttribute("title")).toBe("Screenshot capture shortcut: Ctrl + 1");
    expect(chip?.getAttribute("aria-label")).toBe("Screenshot capture shortcut: Ctrl + 1");
    expect(chip?.querySelector(".screenshot-hotkey-status-icon")?.getAttribute("aria-hidden"))
      .toBe("true");
    expect(chip?.getAttribute("role")).toBeNull();
    expect(chip?.getAttribute("tabindex")).toBeNull();
    expect(chip?.querySelector("button, a")).toBeNull();
    expect(chip?.onclick).toBeNull();
    expect(chip?.onkeydown).toBeNull();
  });

  it.each([
    ["unregistered", screenshotStatus({ registered: false })],
    ["error-bearing", screenshotStatus({ error: "already registered" })],
    ["empty", screenshotStatus({ configured: "" })],
    ["malformed", screenshotStatus({ configured: "Ctrl++1" })],
    ["runtime-malformed registration", { ...screenshotStatus(), registered: "true" }],
  ])("hides a %s status", async (_label, status) => {
    const { root } = await mountTitlebar({ status });

    expect(getChip(root)).toBeNull();
  });

  it("hides a rejected status request without a duplicate warning", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { root } = await mountTitlebar({ rejectStatus: true });

    expect(getChip(root)).toBeNull();
    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });

  it("does not request status in a browser sidebar", async () => {
    const { root } = await mountTitlebar({ native: false });

    expect(getChip(root)).toBeNull();
    expect(mocks.getHotkeyStatus).not.toHaveBeenCalled();
  });

  it("uses the typed status route while preserving the complete native invoke allowlist", async () => {
    await mountTitlebar();

    expect(mocks.getHotkeyStatus).toHaveBeenCalledTimes(1);
    expect(mocks.invoke.mock.calls).toEqual([["get_instance_label"]]);
  });
});
