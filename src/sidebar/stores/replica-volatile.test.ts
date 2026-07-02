import { beforeEach, describe, expect, it } from "vitest";
import { createMemo, createRoot } from "solid-js";
import {
  effectiveAutoClosedAt,
  effectiveLastUserMessageAt,
  effectiveManuallyClosedAt,
  effectiveRepoBranch,
  replicaVolatileStore,
} from "./replica-volatile";

// #748 — the live event layer for the volatile replica fields. These cover the
// contract the old in-place projectStore patches had (#552 normalized-path
// matching, set/clear semantics) plus the new one: reads are reactive and
// fine-grained, so a badge/pill updates WITHOUT re-creating its row.

const COORD_A = "C:\\Users\\Maria\\Project\\.ac\\wg-1-team\\__agent_coord-a";
const COORD_B = "C:\\Users\\Maria\\Project\\.ac\\wg-2-team\\__agent_coord-b";
// Same replica as COORD_A but lower-cased with forward slashes, as the backend
// session working_directory arrives on the clock/auto-close events (#552).
const COORD_A_EVENT = "c:/users/maria/project/.ac/wg-1-team/__agent_coord-a";

describe("replicaVolatileStore (#748)", () => {
  beforeEach(() => {
    replicaVolatileStore.clearAll();
  });

  it("matches event paths to discovery paths by normalization and leaves others untouched", () => {
    replicaVolatileStore.setLastUserMessageAt(COORD_A_EVENT, "2026-06-19T18:00:00Z");

    expect(effectiveLastUserMessageAt({ path: COORD_A })).toBe("2026-06-19T18:00:00Z");
    expect(
      effectiveLastUserMessageAt({ path: COORD_B, lastUserMessageAt: "2020-01-01T00:00:00Z" })
    ).toBe("2020-01-01T00:00:00Z");
  });

  it("falls back to the discovery snapshot when no override exists", () => {
    expect(effectiveAutoClosedAt({ path: COORD_A, autoClosedAt: "2026-06-19T17:00:00Z" })).toBe(
      "2026-06-19T17:00:00Z"
    );
    expect(effectiveRepoBranch({ path: COORD_A, repoBranch: "main" })).toBe("main");
    expect(effectiveManuallyClosedAt({ path: COORD_A })).toBeUndefined();
  });

  it("a null override explicitly masks a stale snapshot marker (reopen clear)", () => {
    const replica = { path: COORD_A, autoClosedAt: "2026-06-19T17:00:00Z" };
    replicaVolatileStore.setAutoClosedAt(COORD_A_EVENT, null);

    expect(effectiveAutoClosedAt(replica)).toBeUndefined();
  });

  it("a null branch override masks the snapshot branch (branch gone)", () => {
    const replica = { path: COORD_A, repoBranch: "main" };
    replicaVolatileStore.setRepoBranch(COORD_A_EVENT, null);

    expect(effectiveRepoBranch(replica)).toBeUndefined();
  });

  it("fields on the same replica override independently (sibling fields preserved)", () => {
    const replica = { path: COORD_A, autoClosedAt: "2026-06-19T17:00:00Z" };
    replicaVolatileStore.setLastUserMessageAt(COORD_A, "2026-06-19T18:00:00Z");

    expect(effectiveLastUserMessageAt(replica)).toBe("2026-06-19T18:00:00Z");
    // The clock write must not disturb the untouched marker field.
    expect(effectiveAutoClosedAt(replica)).toBe("2026-06-19T17:00:00Z");
  });

  it("clearForPaths drops exactly the given paths' overrides (snapshot supersede)", () => {
    replicaVolatileStore.setAutoClosedAt(COORD_A, "2026-06-19T18:05:00Z");
    replicaVolatileStore.setAutoClosedAt(COORD_B, "2026-06-19T18:06:00Z");

    // The reload path clears by the DISCOVERY path shape; overrides were keyed
    // from event-shaped paths — normalization must line them up.
    replicaVolatileStore.clearForPaths([COORD_A_EVENT]);

    expect(effectiveAutoClosedAt({ path: COORD_A })).toBeUndefined();
    expect(effectiveAutoClosedAt({ path: COORD_B })).toBe("2026-06-19T18:06:00Z");
  });

  it("clearAll resets every override", () => {
    replicaVolatileStore.setRepoBranch(COORD_A, "feature/x");
    replicaVolatileStore.setManuallyClosedAt(COORD_B, "2026-06-19T18:07:00Z");

    replicaVolatileStore.clearAll();

    expect(effectiveRepoBranch({ path: COORD_A })).toBeUndefined();
    expect(effectiveManuallyClosedAt({ path: COORD_B })).toBeUndefined();
  });

  it("reads are reactive: a memo re-evaluates on set and clear without any row re-creation", () => {
    createRoot((dispose) => {
      const replica = { path: COORD_A, autoClosedAt: "2026-06-19T17:00:00Z" };
      const marker = createMemo(() => effectiveAutoClosedAt(replica));
      expect(marker()).toBe("2026-06-19T17:00:00Z");

      replicaVolatileStore.setAutoClosedAt(COORD_A_EVENT, null);
      expect(marker()).toBeUndefined();

      replicaVolatileStore.setAutoClosedAt(COORD_A_EVENT, "2026-06-19T19:00:00Z");
      expect(marker()).toBe("2026-06-19T19:00:00Z");

      replicaVolatileStore.clearForPaths([COORD_A]);
      expect(marker()).toBe("2026-06-19T17:00:00Z");

      dispose();
    });
  });
});
