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

describe("sessionsStore.setCommunication (#676)", () => {
  beforeEach(() => {
    sessionsStore.setSessions([]);
  });
  afterEach(() => {
    sessionsStore.setSessions([]);
  });

  it("sets and clears communication on the targeted session", () => {
    sessionsStore.setSessions([
      session({ id: "s1", communication: null }),
      session({ id: "s2", communication: null }),
    ]);

    sessionsStore.setCommunication("s2", {
      kind: "raiseHand",
      visible: true,
      updatedAt: "2026-06-28T17:00:00.000Z",
    });

    expect(sessionsStore.sessions.find((s) => s.id === "s1")?.communication).toBeNull();
    expect(sessionsStore.sessions.find((s) => s.id === "s2")?.communication).toEqual({
      kind: "raiseHand",
      visible: true,
      updatedAt: "2026-06-28T17:00:00.000Z",
    });

    sessionsStore.setCommunication("s2", null);

    expect(sessionsStore.sessions.find((s) => s.id === "s2")?.communication).toBeNull();
  });

  it("preserves frontend-only fields while patching communication", () => {
    sessionsStore.setSessions([
      session({
        id: "s1",
        communication: null,
        pendingReview: true,
        profileOutdated: true,
      }),
    ]);

    sessionsStore.setCommunication("s1", {
      kind: "raiseHand",
      visible: true,
      updatedAt: "2026-06-28T17:00:00.000Z",
    });

    const updated = sessionsStore.sessions.find((s) => s.id === "s1");
    expect(updated?.communication?.kind).toBe("raiseHand");
    expect(updated?.pendingReview).toBe(true);
    expect(updated?.profileOutdated).toBe(true);
  });

  it("ignores an unknown session id", () => {
    sessionsStore.setSessions([
      session({ id: "s1", communication: null }),
    ]);

    sessionsStore.setCommunication("missing", {
      kind: "raiseHand",
      visible: true,
      updatedAt: "2026-06-28T17:00:00.000Z",
    });

    expect(sessionsStore.sessions).toEqual([
      expect.objectContaining({ id: "s1", communication: null }),
    ]);
  });
});
