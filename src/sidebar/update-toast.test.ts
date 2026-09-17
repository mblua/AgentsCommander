import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { createUpdateToaster } from "./update-toast";
import { toastStore } from "../shared/stores/toasts";
import type { UpdateInfo } from "../shared/types";

function info(latestVersion: string): UpdateInfo {
  return {
    currentVersion: "0.9.16",
    latestVersion,
    upgradeCommand: "npm i -g @mblua/agentscommander",
  };
}

describe("createUpdateToaster (#609)", () => {
  beforeEach(() => toastStore.clear());

  it("shows one sticky info toast carrying the version + upgrade command", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toContain("0.9.17");
    expect(toastStore.items[0].message).toContain(
      "npm i -g @mblua/agentscommander",
    );
  });

  it("dedups the same latestVersion (event + snapshot race) to one toast", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));
    show(info("0.9.17"));

    expect(toastStore.items).toHaveLength(1);
  });

  it("shows a second toast when latestVersion changes", () => {
    const show = createUpdateToaster();
    show(info("0.9.17"));
    show(info("0.9.18"));

    expect(toastStore.items).toHaveLength(2);
  });

  it("gives each toaster instance independent dedup state", () => {
    const a = createUpdateToaster();
    const b = createUpdateToaster();
    a(info("0.9.17"));
    b(info("0.9.17"));

    expect(toastStore.items).toHaveLength(2);
  });
});

describe("update toast Copy action (#2135)", () => {
  const COMMAND = "npm i -g @mblua/agentscommander";
  let hadClipboard: boolean;
  let originalClipboard: unknown;

  function setClipboard(value: unknown): void {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value,
    });
  }

  function showToast(): void {
    createUpdateToaster()(info("0.9.17"));
  }

  function clickCopy(): void {
    const action = toastStore.items[0].action;
    expect(action).toBeDefined();
    action?.onClick();
  }

  beforeEach(() => {
    toastStore.clear();
    hadClipboard = "clipboard" in navigator;
    originalClipboard = (navigator as { clipboard?: unknown }).clipboard;
  });

  afterEach(() => {
    if (hadClipboard) {
      setClipboard(originalClipboard);
    } else {
      delete (navigator as { clipboard?: unknown }).clipboard;
    }
    vi.restoreAllMocks();
  });

  it("puts a non-dismissing Copy action on the sticky info toast", () => {
    setClipboard({ writeText: vi.fn().mockResolvedValue(undefined) });
    showToast();

    expect(toastStore.items[0].action?.label).toBe("Copy");
    expect(toastStore.items[0].action?.dismissOnClick).toBe(false);
  });

  it("copies the upgrade command and confirms with a success toast", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    setClipboard({ writeText });
    showToast();
    clickCopy();

    await vi.waitFor(() => {
      expect(toastStore.items).toHaveLength(2);
    });
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith(COMMAND);
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[1].kind).toBe("success");
    expect(toastStore.items[1].message).toBe("Command copied");
  });

  it("keeps the info toast up across repeated copies", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    setClipboard({ writeText });
    showToast();
    clickCopy();
    clickCopy();

    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledTimes(2);
    });
    expect(toastStore.items[0].kind).toBe("info");
    expect(toastStore.items[0].message).toContain(COMMAND);
  });

  it("logs and stays quiet when the clipboard write rejects", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    setClipboard({ writeText: vi.fn().mockRejectedValue(new Error("denied")) });
    showToast();
    expect(() => clickCopy()).not.toThrow();

    await vi.waitFor(() => {
      expect(error).toHaveBeenCalled();
    });
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
  });

  it("logs and stays quiet when navigator.clipboard is missing", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    setClipboard(undefined);
    showToast();
    expect(() => clickCopy()).not.toThrow();

    await vi.waitFor(() => {
      expect(error).toHaveBeenCalled();
    });
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("info");
  });
});
