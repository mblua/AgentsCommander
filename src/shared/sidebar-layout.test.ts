import { describe, expect, it } from "vitest";
import {
  DEFAULT_MAIN_SIDEBAR_WIDTH,
  MAIN_SIDEBAR_MAX_WIDTH,
  MAIN_SIDEBAR_MIN_WIDTH,
  MAIN_TERMINAL_MIN_WIDTH,
  clampMainSidebarWidth,
} from "./sidebar-layout";

describe("main sidebar layout", () => {
  it("defaults above the minimum width needed for the sidebar controls", () => {
    expect(DEFAULT_MAIN_SIDEBAR_WIDTH).toBeGreaterThanOrEqual(MAIN_SIDEBAR_MIN_WIDTH);
    expect(DEFAULT_MAIN_SIDEBAR_WIDTH).toBeLessThanOrEqual(MAIN_SIDEBAR_MAX_WIDTH);
  });

  it("raises stale persisted widths to the current sidebar minimum", () => {
    expect(clampMainSidebarWidth(200, 1400)).toBe(MAIN_SIDEBAR_MIN_WIDTH);
  });

  it("keeps enough room for the terminal pane when clamping wide values", () => {
    const windowWidth = 900;
    expect(clampMainSidebarWidth(800, windowWidth)).toBe(
      windowWidth - MAIN_TERMINAL_MIN_WIDTH,
    );
  });

  it("caps the sidebar width on large windows", () => {
    expect(clampMainSidebarWidth(800, 1600)).toBe(MAIN_SIDEBAR_MAX_WIDTH);
  });
});
