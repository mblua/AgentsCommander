import { beforeEach, describe, expect, it } from "vitest";
import { createMemo, createRoot } from "solid-js";
import {
  effectiveAutoClosedAt,
  effectiveLastUserMessageAt,
  effectiveManuallyClosedAt,
  effectiveRepoBranch,
  effectiveRepoBranchByPath,
  effectiveRepoDirtyByPath,
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

// #943 B2 - `ac_discovery_branch_updated` now also carries per-repo branches.
describe("replicaVolatileStore per-repo branches (#943 B2)", () => {
  const REPO_A = "C:\\proj\\.ac\\wg-1-team\\repo-AgentsCommander";
  const REPO_B = "C:\\proj\\.ac\\wg-1-team\\repo-webpage";

  beforeEach(() => {
    replicaVolatileStore.clearAll();
  });

  it("keys the map by repo path and lands the shorthand in the same write", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      COORD_A_EVENT, // event path: lower-cased, forward slashes (#552)
      null,
      [REPO_A, REPO_B],
      ["feature/943", null]
    );

    // The REPLICA key is normalized; the REPO keys are verbatim.
    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({
      [REPO_A]: "feature/943",
      [REPO_B]: null,
    });
    expect(effectiveRepoBranch({ path: COORD_A })).toBeUndefined();
  });

  it("drops the whole map when the arrays disagree in length", () => {
    // Can only happen if a build breaks the backend's 1:1 invariant. Pairing them
    // anyway would attach a branch to the wrong repo; no branch beats a wrong one.
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A, REPO_B], [
      "only-one",
    ]);

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({});
  });

  it("defaults to an empty map for a replica with no repos", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [], []);

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({});
  });

  it("tolerates a payload from a backend that does not send the arrays", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, "feature/x");

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({});
    expect(effectiveRepoBranch({ path: COORD_A })).toBe("feature/x");
  });

  // H1 (grinch): this test used to assert the OPPOSITE - that clearForPaths drops
  // the map - and that made it the alibi for a HIGH bug. `projectStore` calls
  // clearForPaths on every reload (loop tick, CLI refresh, entity creation, group
  // edit...). The other volatile fields survive that because the discovery snapshot
  // re-supplies them; `repoBranchByPath` has NO snapshot counterpart, so the wipe
  // installed nothing, the badge fell back to the single-repo shorthand (null for a
  // multi-repo replica), and the backend never re-emitted (Gate A: identical payload
  // => no event). Browse Branch died on the first reload and never came back - not
  // for 15s, forever.
  it("PRESERVES the per-repo map across a discovery reload (H1)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["feature/943"]);

    replicaVolatileStore.clearForPaths([COORD_A]);

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({ [REPO_A]: "feature/943" });
  });

  it("still clears the snapshot-backed fields on a reload, map or no map", () => {
    // The "discovery wins on reload" contract must keep holding for every field the
    // snapshot actually re-supplies - preserving the B2 map must not smuggle those
    // back in.
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, "feature/943", [REPO_A], [
      "feature/943",
    ]);
    replicaVolatileStore.setAutoClosedAt(COORD_A, "2026-06-19T18:05:00Z");
    replicaVolatileStore.setLastUserMessageAt(COORD_A, "2026-06-19T18:00:00Z");

    replicaVolatileStore.clearForPaths([COORD_A]);

    expect(effectiveRepoBranch({ path: COORD_A })).toBeUndefined();
    expect(effectiveAutoClosedAt({ path: COORD_A })).toBeUndefined();
    expect(effectiveLastUserMessageAt({ path: COORD_A })).toBeUndefined();
    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({ [REPO_A]: "feature/943" });
  });

  it("clearAll DOES drop the per-repo map (the replicas themselves are gone)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["feature/943"]);

    replicaVolatileStore.clearAll();

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toBeUndefined();
  });

  it("does not leak one replica's per-repo branches to another", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["feature/943"]);

    expect(effectiveRepoBranchByPath({ path: COORD_B })).toBeUndefined();
  });
});

// #1028 - the same `ac_discovery_branch_updated` event now also carries per-repo
// worktree-dirty. It rides the B2 feed exactly, so it inherits B2's by-path merge
// and, critically, B2's clearForPaths PRESERVE.
describe("replicaVolatileStore per-repo dirty (#1028)", () => {
  const REPO_A = "C:\\proj\\.ac\\wg-1-team\\repo-AgentsCommander";
  const REPO_B = "C:\\proj\\.ac\\wg-1-team\\repo-webpage";

  beforeEach(() => {
    replicaVolatileStore.clearAll();
  });

  it("keys dirty by repo path and lands it in the same write as the branches", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      COORD_A_EVENT, // event path: lower-cased, forward slashes (#552)
      null,
      [REPO_A, REPO_B],
      ["feature/1028", null],
      [true, null]
    );

    // The REPLICA key is normalized; the REPO keys are verbatim.
    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toEqual({
      [REPO_A]: true,
      [REPO_B]: null,
    });
  });

  // The `?? null` vs `|| null` guard in zipByPath, and the ONLY reason this test can
  // exist as a distinct case: `false` and `null` are byte-identical on the badge
  // (both violet). If `false` collapsed to `null` here, nothing on screen would move
  // and the only symptom would be every clean repo's tooltip reading "(status
  // unknown)". Nothing else in the suite would notice.
  it("preserves an explicit `false` (detected clean) instead of collapsing it to null", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], [null], [false]);

    const map = effectiveRepoDirtyByPath({ path: COORD_A });
    expect(map).toEqual({ [REPO_A]: false });
    // toEqual would pass on `undefined` too; pin the exact value.
    expect(map?.[REPO_A]).toBe(false);
  });

  it("drops ONLY the dirty map when the dirty array disagrees in length", () => {
    // The length guard is applied PER call, so a malformed `repoDirty` from a build
    // that broke the backend's 1:1 invariant cannot take the branch map down with it.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      COORD_A,
      null,
      [REPO_A, REPO_B],
      ["branch-a", "branch-b"],
      [true] // 1 dirty for 2 repos
    );

    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toEqual({});
    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({
      [REPO_A]: "branch-a",
      [REPO_B]: "branch-b",
    });
  });

  it("tolerates a payload from a backend that does not send the dirty array", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["feature/x"]);

    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toEqual({});
    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({ [REPO_A]: "feature/x" });
  });

  // AC 6, and the highest-risk item in the change. Same failure mode the B2 map has a
  // HIGH bug on record for: `projectStore` calls clearForPaths on EVERY reload (loop
  // tick, CLI refresh, entity creation), `repoDirtyByPath` has no `AcAgentReplica`
  // counterpart to re-supply it, and Gate A only emits on a CHANGED payload - so a
  // wiped map is never re-sent. The red would vanish on the first reload and not come
  // back until the worktree changed on disk or the app restarted.
  it("PRESERVES the dirty map across a discovery reload (AC 6)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["main"], [true]);

    replicaVolatileStore.clearForPaths([COORD_A]);

    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toEqual({ [REPO_A]: true });
  });

  it("preserves BOTH maps across a reload, without smuggling back the snapshot fields", () => {
    // The "discovery wins on reload" contract must keep holding for every field the
    // snapshot re-supplies; preserving two maps must not resurrect a third field.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      COORD_A,
      "feature/1028",
      [REPO_A],
      ["feature/1028"],
      [false]
    );
    replicaVolatileStore.setAutoClosedAt(COORD_A, "2026-07-16T18:05:00Z");

    replicaVolatileStore.clearForPaths([COORD_A]);

    expect(effectiveRepoBranchByPath({ path: COORD_A })).toEqual({ [REPO_A]: "feature/1028" });
    // A preserved `false` must stay `false`, not decay to "never detected".
    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toEqual({ [REPO_A]: false });
    expect(effectiveRepoBranch({ path: COORD_A })).toBeUndefined();
    expect(effectiveAutoClosedAt({ path: COORD_A })).toBeUndefined();
  });

  it("clearAll DOES drop the dirty map (the replicas themselves are gone)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["main"], [true]);

    replicaVolatileStore.clearAll();

    expect(effectiveRepoDirtyByPath({ path: COORD_A })).toBeUndefined();
  });

  it("does not leak one replica's dirty map to another", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A, null, [REPO_A], ["main"], [true]);

    expect(effectiveRepoDirtyByPath({ path: COORD_B })).toBeUndefined();
  });

  it("reads are reactive: a memo re-evaluates when dirty flips, without a row re-creation", () => {
    createRoot((dispose) => {
      const replica = { path: COORD_A };
      const dirty = createMemo(() => effectiveRepoDirtyByPath(replica)?.[REPO_A]);
      expect(dirty()).toBeUndefined();

      replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A_EVENT, null, [REPO_A], ["main"], [
        true,
      ]);
      expect(dirty()).toBe(true);

      replicaVolatileStore.applyDiscoveryBranchUpdate(COORD_A_EVENT, null, [REPO_A], ["main"], [
        false,
      ]);
      expect(dirty()).toBe(false);

      dispose();
    });
  });
});
