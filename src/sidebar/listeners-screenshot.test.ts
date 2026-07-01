import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests } from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";
import type { ScreenshotHotkeyStatus } from "../shared/types";
import { wireScreenshotListeners } from "./listeners-screenshot";

const OK_STATUS: ScreenshotHotkeyStatus = {
  configured: "Ctrl+Q",
  registered: true,
  error: null,
};

describe("wireScreenshotListeners", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
  });

  afterEach(() => {
    toastStore.clear();
    restore();
    vi.restoreAllMocks();
  });

  it("pushes a sticky success toast carrying the saved path on screenshot_capture_saved", async () => {
    fake.resolve("screenshot_get_hotkey_status", OK_STATUS);
    const unlisteners = await wireScreenshotListeners();

    const path = "C:\\proj\\.ac\\wg-6-dev-team\\__agent_x\\agentscommander-screenshot.png";
    fake.emitFromBackend("screenshot_capture_saved", {
      path,
      sessionId: "s1",
      sessionName: "wg-6-dev-team/dev-webpage-ui",
    });

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("success");
    expect(toastStore.items[0].message).toContain(path);

    unlisteners.forEach((u) => u());
  });

  it("pushes a sticky error toast with the backend message on screenshot_capture_failed", async () => {
    fake.resolve("screenshot_get_hotkey_status", OK_STATUS);
    const unlisteners = await wireScreenshotListeners();

    fake.emitFromBackend("screenshot_capture_failed", {
      message: "No active session to save the screenshot into.",
    });

    const errors = toastStore.items.filter((t) => t.kind === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0].message).toBe(
      "No active session to save the screenshot into."
    );

    unlisteners.forEach((u) => u());
  });

  it("warns once at startup when the hotkey is configured but not registered", async () => {
    fake.resolve("screenshot_get_hotkey_status", {
      configured: "Ctrl+Q",
      registered: false,
      error: "already registered by another application",
    } satisfies ScreenshotHotkeyStatus);

    const unlisteners = await wireScreenshotListeners();

    await vi.waitFor(() => {
      const warning = toastStore.items.find(
        (t) => t.kind === "error" && t.message.includes("is not active")
      );
      expect(warning).toBeTruthy();
      expect(warning!.message).toContain("Ctrl+Q");
      expect(warning!.message).toContain(
        "already registered by another application"
      );
    });

    unlisteners.forEach((u) => u());
  });

  it("does NOT warn at startup when the hotkey registered cleanly", async () => {
    fake.resolve("screenshot_get_hotkey_status", OK_STATUS);
    const unlisteners = await wireScreenshotListeners();

    // Let the fire-and-forget status check settle.
    await vi.waitFor(() => {
      expect(fake.callsFor("screenshot_get_hotkey_status")).toHaveLength(1);
    });
    // A short microtask flush; no toast should have been added.
    await Promise.resolve();
    expect(toastStore.items).toHaveLength(0);

    unlisteners.forEach((u) => u());
  });
});
