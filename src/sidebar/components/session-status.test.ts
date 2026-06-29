import { describe, expect, it } from "vitest";
import { sessionDotClass } from "./session-status";
import type { SessionStatus } from "../../shared/types";

function dotSession(
  status: SessionStatus,
  flags: { pendingReview?: boolean; waitingForInput?: boolean } = {},
) {
  return {
    status,
    pendingReview: flags.pendingReview ?? false,
    waitingForInput: flags.waitingForInput ?? false,
  };
}

describe("sessionDotClass", () => {
  it("uses exited for dormant sessions even when stale live flags remain", () => {
    expect(sessionDotClass(dotSession({ exited: 0 }, { waitingForInput: true }))).toBe("exited");
    expect(sessionDotClass(dotSession({ exited: 0 }, { pendingReview: true }))).toBe("exited");
  });

  it("preserves live pending/waiting overrides and runtime states", () => {
    expect(sessionDotClass(dotSession("running", { waitingForInput: true }))).toBe("waiting");
    expect(sessionDotClass(dotSession("running", { pendingReview: true }))).toBe("pending");
    expect(sessionDotClass(dotSession("running"))).toBe("running");
    expect(sessionDotClass(dotSession("active"))).toBe("active");
  });

  it("uses offline for missing or inactive rows", () => {
    expect(sessionDotClass(null)).toBe("offline");
    expect(sessionDotClass(dotSession("running"), { inactive: true })).toBe("offline");
  });
});
