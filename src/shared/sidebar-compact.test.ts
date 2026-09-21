// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_SIDEBAR_COMPACT_HOTKEY,
  RAIL_WIDTH_PX,
  currentHotkey,
  hasCompactHost,
  railNudgePx,
  registerCompactHost,
  restoreWidthPx,
  setRailNudgePx,
  setRestoreWidthPx,
  setSidebarCompactHotkey,
  setSidebarCompactMode,
  sidebarCompact,
  toggleSidebarCompact,
} from "./sidebar-compact";
import { resetSidebarCompactForTests } from "./testing/ui-harness";

describe("sidebar compact signal (#2236)", () => {
  beforeEach(() => {
    resetSidebarCompactForTests();
  });

  it("defaults to expanded and flips only through the entry point with a host", () => {
    const hook = vi.fn();
    const release = registerCompactHost({ onBeforeModeChange: hook });
    try {
      expect(sidebarCompact()).toBe(false);
      toggleSidebarCompact();
      expect(sidebarCompact()).toBe(true);
      expect(hook).toHaveBeenCalledTimes(1);
      expect(hook).toHaveBeenCalledWith(true);

      // Same-value call is a no-op and fires no hook.
      setSidebarCompactMode(true);
      expect(sidebarCompact()).toBe(true);
      expect(hook).toHaveBeenCalledTimes(1);
    } finally {
      release();
    }
  });

  it("does nothing without a registered host (D11)", () => {
    const hook = vi.fn();
    const release = registerCompactHost({ onBeforeModeChange: hook });
    release();
    expect(hasCompactHost()).toBe(false);

    toggleSidebarCompact();
    expect(sidebarCompact()).toBe(false);
    setSidebarCompactMode(true);
    expect(sidebarCompact()).toBe(false);
    expect(hook).not.toHaveBeenCalled();
  });

  it("is reference-counted per registration and releases hooks idempotently", () => {
    const first = vi.fn();
    const second = vi.fn();
    const releaseFirst = registerCompactHost({ onBeforeModeChange: first });
    const releaseSecond = registerCompactHost({ onBeforeModeChange: second });
    try {
      releaseFirst();
      releaseFirst();
      expect(hasCompactHost()).toBe(true);

      setSidebarCompactMode(true);
      expect(second).toHaveBeenCalledWith(true);
      expect(first).not.toHaveBeenCalled();
    } finally {
      releaseSecond();
    }
    expect(hasCompactHost()).toBe(false);
  });

  it("pins RAIL_WIDTH_PX to the --ac-rail-width token phase 1 landed", () => {
    expect(RAIL_WIDTH_PX).toBe(68);
    // Vite rewrites the literal form of `new URL(..., import.meta.url)` into a
    // served asset URL (http://localhost:3000/...) under jsdom; the variable base
    // keeps the real file: URL, which node:fs accepts (AgentUpdateOverlay.test.tsx).
    const moduleUrl = import.meta.url;
    const css = readFileSync(
      new URL("../sidebar/styles/variables.css", moduleUrl),
      "utf8",
    );
    const tokenLine = css
      .split(/\r?\n/)
      .find((line) => line.includes("--ac-rail-width"));
    const match = tokenLine
      ? /--ac-rail-width\s*:\s*(\d+)px/.exec(tokenLine)
      : null;
    if (!match) {
      throw new Error(
        "--ac-rail-width token missing from src/sidebar/styles/variables.css",
      );
    }
    expect(Number(match[1])).toBe(RAIL_WIDTH_PX);
  });

  it("runs every hook before the flip, while the outgoing mode still reads (D21)", () => {
    const observed: Array<{ current: boolean; next: boolean }> = [];
    const release = registerCompactHost({
      onBeforeModeChange: (next) => {
        observed.push({ current: sidebarCompact(), next });
      },
    });
    try {
      setSidebarCompactMode(true);
      expect(observed).toEqual([{ current: false, next: true }]);
      setSidebarCompactMode(false);
      expect(observed).toEqual([
        { current: false, next: true },
        { current: true, next: false },
      ]);
    } finally {
      release();
    }
  });

  it("defaults the nudge and snapshot to 0 and resets them", () => {
    expect(railNudgePx()).toBe(0);
    expect(restoreWidthPx()).toBe(0);
    setRailNudgePx(16);
    setRestoreWidthPx(520);
    expect(railNudgePx()).toBe(16);
    expect(restoreWidthPx()).toBe(520);

    resetSidebarCompactForTests();
    expect(railNudgePx()).toBe(0);
    expect(restoreWidthPx()).toBe(0);
  });

  it("defaults the hotkey and restores it on reset", () => {
    expect(DEFAULT_SIDEBAR_COMPACT_HOTKEY).toBe("Ctrl+Shift+E");
    expect(currentHotkey()).toBe(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
    setSidebarCompactHotkey("Ctrl+Alt+K");
    expect(currentHotkey()).toBe("Ctrl+Alt+K");

    resetSidebarCompactForTests();
    expect(currentHotkey()).toBe(DEFAULT_SIDEBAR_COMPACT_HOTKEY);
  });

  it("does not export the raw signal setter", async () => {
    const moduleNamespace = await import("./sidebar-compact");
    expect(Object.keys(moduleNamespace)).not.toContain("setSidebarCompactSignal");
    expect("setSidebarCompactSignal" in moduleNamespace).toBe(false);
  });
});
