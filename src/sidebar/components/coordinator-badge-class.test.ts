import { describe, expect, it } from "vitest";
import type { AppSettings } from "../../shared/types";
import { coordinatorIdleBadge } from "../../shared/coordinator-badge";
import { COORD_IDLE_CLASS } from "./coordinator-badge-class";

const NOW = Date.parse("2026-06-18T12:00:00.000Z");

function minsAgo(mins: number): string {
  return new Date(NOW - mins * 60_000).toISOString();
}

function thresholds(
  yellow: number,
  red: number
): Pick<
  AppSettings,
  "coordinatorIdleBadgeYellowMinutes" | "coordinatorIdleBadgeRedMinutes"
> {
  return {
    coordinatorIdleBadgeYellowMinutes: yellow,
    coordinatorIdleBadgeRedMinutes: red,
  };
}

describe("COORD_IDLE_CLASS", () => {
  it("maps coordinator idle levels to the existing CSS classes", () => {
    expect(COORD_IDLE_CLASS).toEqual({ ok: "green", warn: "yellow", stale: "red" });
  });

  it("preserves the pre-#882 minute boundary class behavior", () => {
    const t = thresholds(30, 60);

    expect(COORD_IDLE_CLASS[coordinatorIdleBadge(minsAgo(29), NOW, t)!.level]).toBe("green");
    expect(COORD_IDLE_CLASS[coordinatorIdleBadge(minsAgo(30), NOW, t)!.level]).toBe("yellow");
    expect(COORD_IDLE_CLASS[coordinatorIdleBadge(minsAgo(60), NOW, t)!.level]).toBe("red");
    expect(COORD_IDLE_CLASS[coordinatorIdleBadge(minsAgo(600), NOW, t)!.level]).toBe("red");
  });

  it("is exhaustive over all coordinator idle levels", () => {
    expect(Object.keys(COORD_IDLE_CLASS).sort()).toEqual(["ok", "stale", "warn"]);
  });
});
