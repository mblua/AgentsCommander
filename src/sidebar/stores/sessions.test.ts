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

describe("sessionsStore context readings (#1033)", () => {
  beforeEach(() => {
    sessionsStore.setSessions([]);
  });
  afterEach(() => {
    sessionsStore.setSessions([]);
  });

  it("stores a reading an event carried (an_event_sets_a_sessions_reading)", () => {
    sessionsStore.setSessionContext("ctx-a", 42);

    expect(sessionsStore.contextPercentBySessionId["ctx-a"]).toBe(42);
  });

  it("keeps two sessions independent (two_sessions_never_cross)", () => {
    sessionsStore.setSessionContext("ctx-b1", 42);
    sessionsStore.setSessionContext("ctx-b2", 7);

    expect(sessionsStore.contextPercentBySessionId["ctx-b1"]).toBe(42);
    expect(sessionsStore.contextPercentBySessionId["ctx-b2"]).toBe(7);
  });

  it("stores an explicit null rather than dropping the key (a_null_is_stored_as_null_not_dropped)", () => {
    // null is the engine's answer, not an absence: it must overwrite a stale reading.
    sessionsStore.setSessionContext("ctx-c", 42);
    sessionsStore.setSessionContext("ctx-c", null);

    expect("ctx-c" in sessionsStore.contextPercentBySessionId).toBe(true);
    expect(sessionsStore.contextPercentBySessionId["ctx-c"]).toBeNull();
  });

  it("seeds a session no event has spoken for (hydrate_seeds_a_session_no_event_has_spoken_for)", () => {
    sessionsStore.hydrateSessionContext("ctx-d", 42);

    expect(sessionsStore.contextPercentBySessionId["ctx-d"]).toBe(42);
  });

  // The ordering rule: App.tsx registers the listener BEFORE hydrating, so a slow
  // invoke must never overwrite a fresher event that already landed.
  it("never clobbers a value an event already set (hydrate_never_clobbers_a_value_an_event_already_set)", () => {
    sessionsStore.setSessionContext("ctx-e", 43);

    sessionsStore.hydrateSessionContext("ctx-e", 42);

    expect(sessionsStore.contextPercentBySessionId["ctx-e"]).toBe(43);
  });

  // Red if the guard is a truthiness check instead of a key-presence check.
  it("treats a stored zero as spoken for (hydrate_treats_a_stored_zero_as_spoken_for)", () => {
    sessionsStore.setSessionContext("ctx-f", 0);

    sessionsStore.hydrateSessionContext("ctx-f", 42);

    expect(sessionsStore.contextPercentBySessionId["ctx-f"]).toBe(0);
  });

  // A stored null is an answer too, so hydration must not "fill it in".
  it("treats a stored null as spoken for (hydrate_treats_a_stored_null_as_spoken_for)", () => {
    sessionsStore.setSessionContext("ctx-g", null);

    sessionsStore.hydrateSessionContext("ctx-g", 42);

    expect(sessionsStore.contextPercentBySessionId["ctx-g"]).toBeNull();
  });

  // The map lives outside state.sessions on purpose: setSessions is a wholesale
  // replace with no field preservation, and a reading on Session would be wiped.
  it("survives setSessions's wholesale replace (setSessions_cannot_wipe_a_reading)", () => {
    sessionsStore.setSessionContext("ctx-h", 42);

    sessionsStore.setSessions([session({ id: "ctx-h" })]);

    expect(sessionsStore.contextPercentBySessionId["ctx-h"]).toBe(42);
  });
});
