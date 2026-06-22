// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { sessionsStore } from "./sessions";
import { session } from "../../shared/testing/ui-harness";

describe("sessionsStore.setProfileOutdated (#592)", () => {
  beforeEach(() => {
    sessionsStore.setSessions([]);
  });
  afterEach(() => {
    sessionsStore.setSessions([]);
  });

  it("patches only profileOutdated and leaves pendingReview untouched", () => {
    sessionsStore.setSessions([
      session({ id: "s1", pendingReview: true, profileOutdated: false }),
    ]);

    sessionsStore.setProfileOutdated("s1", true);

    const updated = sessionsStore.sessions.find((s) => s.id === "s1");
    expect(updated?.profileOutdated).toBe(true);
    // The frontend-only review flag must survive a surgical drift update.
    expect(updated?.pendingReview).toBe(true);
  });

  it("only touches the targeted session", () => {
    sessionsStore.setSessions([
      session({ id: "s1", profileOutdated: false }),
      session({ id: "s2", profileOutdated: false }),
    ]);

    sessionsStore.setProfileOutdated("s2", true);

    expect(sessionsStore.sessions.find((s) => s.id === "s1")?.profileOutdated).toBe(false);
    expect(sessionsStore.sessions.find((s) => s.id === "s2")?.profileOutdated).toBe(true);
  });
});
