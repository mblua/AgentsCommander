import { describe, expect, it } from "vitest";
import {
  GENERAL_CATEGORIES,
  GENERAL_SETTINGS_INDEX,
  countByCategory,
  searchGeneralSettings,
} from "./generalSettingsIndex";

const keys = (q: string) => searchGeneralSettings(q).map((e) => e.key);

describe("generalSettingsIndex (#2704)", () => {
  it("has unique keys and only known categories", () => {
    const all = GENERAL_SETTINGS_INDEX.map((e) => e.key);
    expect(new Set(all).size).toBe(all.length);
    const ids = new Set(GENERAL_CATEGORIES.map((c) => c.id));
    for (const e of GENERAL_SETTINGS_INDEX) expect(ids.has(e.category)).toBe(true);
  });

  it("returns nothing for an empty or blank query", () => {
    expect(searchGeneralSettings("")).toEqual([]);
    expect(searchGeneralSettings("   ")).toEqual([]);
  });

  it("matches case-insensitively", () => {
    expect(keys("PORT")).toContain("apiServerPort");
  });

  it("requires every token to match", () => {
    expect(keys("orchestrator badge red")).toEqual(["coordinatorIdleBadgeRedMinutes"]);
  });

  it("matches section and category names", () => {
    expect(keys("hotkeys")).toEqual(["screenshotCaptureHotkey", "sidebarCompactHotkey"]);
    const network = GENERAL_SETTINGS_INDEX.filter((e) => e.category === "network").map((e) => e.key);
    expect(keys("network")).toEqual(network);
  });

  it("returns nothing when no entry matches", () => {
    expect(searchGeneralSettings("zzqx")).toEqual([]);
  });

  it("keeps index order", () => {
    const results = searchGeneralSettings("e");
    const positions = results.map((r) => GENERAL_SETTINGS_INDEX.indexOf(r));
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
    expect(results.length).toBeGreaterThan(1);
  });

  it("counts results per category", () => {
    expect(countByCategory(searchGeneralSettings("hotkey"))).toEqual({
      appearance: 2,
      terminal: 0,
      agents: 0,
      network: 0,
      system: 0,
    });
  });
});
