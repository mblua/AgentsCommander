import { describe, expect, it } from "vitest";
import type { SessionStatus } from "./types";
import {
  isSessionWorking,
  isWorkingActivity,
  sessionActivity,
  type ActivitySession,
  type SessionActivity,
} from "./session-activity";
import { sessionDotClass } from "../sidebar/components/session-status";

function activitySession(
  status: SessionStatus,
  flags: { pendingReview?: boolean; waitingForInput?: boolean } = {}
): ActivitySession {
  return {
    status,
    pendingReview: flags.pendingReview ?? false,
    waitingForInput: flags.waitingForInput ?? false,
  };
}

describe("sessionActivity", () => {
  it("uses the same precedence as the visual dot classifier", () => {
    expect(sessionActivity(activitySession({ exited: 0 }, { waitingForInput: true }))).toBe("exited");
    expect(sessionActivity(activitySession({ exited: 0 }, { pendingReview: true }))).toBe("exited");
    expect(sessionActivity(activitySession("running", { waitingForInput: true }))).toBe("waitingForInput");
    expect(sessionActivity(activitySession("running", { pendingReview: true }))).toBe("pendingReview");
    expect(sessionActivity(activitySession("running"))).toBe("running");
    expect(sessionActivity(activitySession("active"))).toBe("active");
    expect(sessionActivity(activitySession("idle"))).toBe("idle");
    expect(sessionActivity(null)).toBe("offline");
    expect(sessionActivity(activitySession("running"), { inactive: true })).toBe("offline");
  });

  it("pins pendingReview above waitingForInput when both flags are true", () => {
    // #882/#886: sessions.ts currently lowers these flags in separate writes.
    // Reading pendingReview first keeps pending consumers from subscribing to
    // waitingForInput, so this order is behavioral, not visual.
    const both = activitySession("running", {
      pendingReview: true,
      waitingForInput: true,
    });

    expect(sessionActivity(both)).toBe("pendingReview");
    expect(sessionDotClass(both)).toBe("pending");
  });

  it("pins every domain activity to its visual dot projection", () => {
    const cases: Record<SessionActivity, { session: ActivitySession | null; inactive?: boolean; dot: string }> = {
      offline: { session: null, dot: "offline" },
      exited: { session: activitySession({ exited: 0 }), dot: "exited" },
      pendingReview: { session: activitySession("running", { pendingReview: true }), dot: "pending" },
      waitingForInput: { session: activitySession("running", { waitingForInput: true }), dot: "waiting" },
      active: { session: activitySession("active"), dot: "active" },
      running: { session: activitySession("running"), dot: "running" },
      idle: { session: activitySession("idle"), dot: "idle" },
    };

    for (const [activity, spec] of Object.entries(cases) as Array<[
      SessionActivity,
      { session: ActivitySession | null; inactive?: boolean; dot: string },
    ]>) {
      expect(sessionActivity(spec.session, { inactive: spec.inactive })).toBe(activity);
      expect(sessionDotClass(spec.session, { inactive: spec.inactive })).toBe(spec.dot);
    }

    expect(sessionDotClass(activitySession("running"), { inactive: true })).toBe("offline");
  });

  it("classifies only active and running as working", () => {
    const expected: Record<SessionActivity, boolean> = {
      offline: false,
      exited: false,
      pendingReview: false,
      waitingForInput: false,
      active: true,
      running: true,
      idle: false,
    };

    for (const [activity, working] of Object.entries(expected) as Array<[SessionActivity, boolean]>) {
      expect(isWorkingActivity(activity)).toBe(working);
    }
  });

  it("keeps the working predicate aligned with the dot-class working projection", () => {
    const statuses: SessionStatus[] = ["active", "running", "idle", { exited: 0 }];
    const flags = [false, true];
    const workingDots = new Set(["active", "running"]);

    for (const status of statuses) {
      for (const pendingReview of flags) {
        for (const waitingForInput of flags) {
          const s = activitySession(status, { pendingReview, waitingForInput });
          expect(isSessionWorking(s)).toBe(workingDots.has(sessionDotClass(s)));
        }
      }
    }

    expect(isSessionWorking(null)).toBe(workingDots.has(sessionDotClass(null)));
  });
});
