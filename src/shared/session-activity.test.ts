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

  // #2271 - Co-managed precedence, one test per decision the epic fixes.
  it("comanaged beats waitingForInput (test 2)", () => {
    const waiting = activitySession("running", { waitingForInput: true });
    expect(sessionActivity(waiting, { comanaged: true })).toBe("comanaged");
    expect(sessionDotClass(waiting, { comanaged: true })).toBe("comanaged");
  });

  it("comanaged beats pendingReview (test 3)", () => {
    const pending = activitySession("running", { pendingReview: true });
    expect(sessionActivity(pending, { comanaged: true })).toBe("comanaged");
    expect(sessionDotClass(pending, { comanaged: true })).toBe("comanaged");
  });

  it("comanaged beats both flags at once (test 4)", () => {
    const both = activitySession("running", { pendingReview: true, waitingForInput: true });
    expect(sessionActivity(both, { comanaged: true })).toBe("comanaged");
    expect(sessionDotClass(both, { comanaged: true })).toBe("comanaged");
  });

  it("exited beats comanaged (test 5)", () => {
    const dead = activitySession({ exited: 0 }, { waitingForInput: true, pendingReview: true });
    expect(sessionActivity(dead, { comanaged: true })).toBe("exited");
    expect(sessionDotClass(dead, { comanaged: true })).toBe("exited");
  });

  it("offline beats comanaged (test 6)", () => {
    expect(sessionActivity(null, { comanaged: true })).toBe("offline");
    expect(sessionActivity(activitySession("running"), { inactive: true, comanaged: true })).toBe("offline");
    expect(sessionDotClass(null, { comanaged: true })).toBe("offline");
    expect(sessionDotClass(activitySession("running"), { inactive: true, comanaged: true })).toBe("offline");
  });

  it("pins every domain activity to its visual dot projection", () => {
    const cases: Record<SessionActivity, { session: ActivitySession | null; inactive?: boolean; comanaged?: boolean; dot: string }> = {
      offline: { session: null, dot: "offline" },
      exited: { session: activitySession({ exited: 0 }), dot: "exited" },
      // #2271 - totality: the new variant maps 1:1 like every other one.
      comanaged: { session: activitySession("running"), comanaged: true, dot: "comanaged" },
      pendingReview: { session: activitySession("running", { pendingReview: true }), dot: "pending" },
      waitingForInput: { session: activitySession("running", { waitingForInput: true }), dot: "waiting" },
      active: { session: activitySession("active"), dot: "active" },
      running: { session: activitySession("running"), dot: "running" },
      idle: { session: activitySession("idle"), dot: "idle" },
    };

    for (const [activity, spec] of Object.entries(cases) as Array<[
      SessionActivity,
      { session: ActivitySession | null; inactive?: boolean; comanaged?: boolean; dot: string },
    ]>) {
      expect(sessionActivity(spec.session, { inactive: spec.inactive, comanaged: spec.comanaged })).toBe(activity);
      expect(sessionDotClass(spec.session, { inactive: spec.inactive, comanaged: spec.comanaged })).toBe(spec.dot);
    }

    expect(sessionDotClass(activitySession("running"), { inactive: true })).toBe("offline");
  });

  it("classifies only active and running as working", () => {
    const expected: Record<SessionActivity, boolean> = {
      offline: false,
      exited: false,
      comanaged: false,
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
