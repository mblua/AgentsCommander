import { describe, expect, it } from "vitest";
import { coordinatorIdleBadge } from "./coordinator-badge";
import type { AppSettings } from "./types";

const NOW = Date.parse("2026-06-18T12:00:00.000Z");

/** ISO string for `mins` minutes before NOW. */
function minsAgo(mins: number): string {
  return new Date(NOW - mins * 60_000).toISOString();
}

/** Minimal settings slice carrying just the two threshold fields. */
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

describe("coordinatorIdleBadge", () => {
  it("returns null when no timestamp is provided", () => {
    expect(coordinatorIdleBadge(undefined, NOW, thresholds(30, 60))).toBeNull();
  });

  it("returns null for an unparseable timestamp", () => {
    expect(coordinatorIdleBadge("not-a-date", NOW, thresholds(30, 60))).toBeNull();
  });

  it("formats minutes only, raw, with no h/d rollover", () => {
    expect(coordinatorIdleBadge(minsAgo(0), NOW, thresholds(30, 60))?.label).toBe("0m");
    expect(coordinatorIdleBadge(minsAgo(45), NOW, thresholds(30, 60))?.label).toBe("45m");
    expect(coordinatorIdleBadge(minsAgo(135), NOW, thresholds(30, 60))?.label).toBe("135m");
    // 3 days = 4320 minutes; stays raw with no day/hour rollover.
    expect(coordinatorIdleBadge(minsAgo(4320), NOW, thresholds(30, 60))?.label).toBe("4320m");
  });

  it("applies green/yellow/red exactly at the default 30/60 boundaries", () => {
    const t = thresholds(30, 60);
    expect(coordinatorIdleBadge(minsAgo(0), NOW, t)?.colorClass).toBe("green");
    expect(coordinatorIdleBadge(minsAgo(29), NOW, t)?.colorClass).toBe("green"); // yellow - 1
    expect(coordinatorIdleBadge(minsAgo(30), NOW, t)?.colorClass).toBe("yellow"); // == yellow
    expect(coordinatorIdleBadge(minsAgo(59), NOW, t)?.colorClass).toBe("yellow"); // red - 1
    expect(coordinatorIdleBadge(minsAgo(60), NOW, t)?.colorClass).toBe("red"); // == red
    expect(coordinatorIdleBadge(minsAgo(600), NOW, t)?.colorClass).toBe("red");
  });

  it("honors custom thresholds", () => {
    const t = thresholds(5, 10);
    expect(coordinatorIdleBadge(minsAgo(4), NOW, t)?.colorClass).toBe("green");
    expect(coordinatorIdleBadge(minsAgo(5), NOW, t)?.colorClass).toBe("yellow");
    expect(coordinatorIdleBadge(minsAgo(9), NOW, t)?.colorClass).toBe("yellow");
    expect(coordinatorIdleBadge(minsAgo(10), NOW, t)?.colorClass).toBe("red");
  });

  it("falls back to default 30/60 thresholds when settings is null", () => {
    expect(coordinatorIdleBadge(minsAgo(29), NOW, null)?.colorClass).toBe("green");
    expect(coordinatorIdleBadge(minsAgo(30), NOW, null)?.colorClass).toBe("yellow");
    expect(coordinatorIdleBadge(minsAgo(60), NOW, null)?.colorClass).toBe("red");
  });

  it("floors a future-dated timestamp to 0m green (clock skew), never negative", () => {
    const future = new Date(NOW + 5 * 60_000).toISOString();
    const badge = coordinatorIdleBadge(future, NOW, thresholds(30, 60));
    expect(badge?.label).toBe("0m");
    expect(badge?.colorClass).toBe("green");
  });
});
