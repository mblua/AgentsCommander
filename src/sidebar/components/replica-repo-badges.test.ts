import { beforeEach, describe, expect, it } from "vitest";
import {
  automationIdPart,
  configuredReplicaRepoBadges,
  formatReplicaRepoBadgeLabel,
  formatReplicaRepoBadgeTitle,
  repoLabelFromPath,
} from "./replica-repo-badges";
import {
  effectiveRepoBranchByPath,
  effectiveRepoDirtyByPath,
  replicaVolatileStore,
} from "../stores/replica-volatile";

describe("replica repo badges", () => {
  it("renders every configured repo path for dormant coordinator rows", () => {
    const badges = configuredReplicaRepoBadges(
      {
        repoPaths: [
          "../repo-Hello-World",
          "../repo-Spoon-Knife",
          "../repo-octocat.github.io",
        ],
        repoBranch: "main",
      },
      { repoPath: "../repo-Hello-World" }
    );

    expect(badges.map(formatReplicaRepoBadgeLabel)).toEqual([
      "Hello-World",
      "Spoon-Knife",
      "octocat.github.io",
    ]);
    expect(badges.map((badge) => badge.branch)).toEqual([null, null, null]);
  });

  it("keeps branch context for a single configured repo path", () => {
    const badges = configuredReplicaRepoBadges(
      {
        repoPaths: ["C:\\work\\repo-Hello-World"],
        repoBranch: "feature/repo-badges",
      },
      { repoPath: undefined }
    );

    expect(badges.map(formatReplicaRepoBadgeLabel)).toEqual([
      "Hello-World/feature/repo-badges",
    ]);
  });

  it("falls back to the workgroup repo path for legacy single-repo discovery data", () => {
    const badges = configuredReplicaRepoBadges(
      { repoPaths: [], repoBranch: "main" },
      { repoPath: "C:/work/repo-Spoon-Knife" }
    );

    expect(badges.map(formatReplicaRepoBadgeLabel)).toEqual(["Spoon-Knife/main"]);
  });

  it("normalizes labels and automation id parts", () => {
    expect(repoLabelFromPath("C:/work/repo-octocat.github.io/")).toBe("octocat.github.io");
    expect(automationIdPart("wg 1/badge main")).toBe("wg-1-badge-main");
  });
});

// #943 B2 - a cold multi-repo replica used to report `branch: null` for every
// repo (discovery only detects a branch when repo_paths.len() == 1), so it could
// never show Browse Branch. The watcher already computed the per-repo branches
// and threw them away; B2 keeps them. These tests drive the real seam the sidebar
// uses: replicaVolatileStore <- event, then configuredReplicaRepoBadges reads it.
describe("#943 B2 per-repo branches are merged BY PATH", () => {
  const REPLICA = "C:\\proj\\.ac\\wg-1-team\\__agent_coord";
  const REPO_A = "C:\\proj\\.ac\\wg-1-team\\repo-AgentsCommander";
  const REPO_B = "C:\\proj\\.ac\\wg-1-team\\repo-webpage";
  const REPO_C = "C:\\proj\\.ac\\wg-1-team\\repo-docs";

  beforeEach(() => {
    replicaVolatileStore.clearAll();
  });

  /** Exactly what configuredReplicaRepoBadgesLive (ProjectPanel) passes. */
  const badgesFor = (repoPaths: string[], repoBranch?: string) =>
    configuredReplicaRepoBadges(
      {
        repoPaths,
        repoBranch,
        repoBranchByPath: effectiveRepoBranchByPath({ path: REPLICA }),
      },
      { repoPath: undefined }
    );

  it("gives a cold multi-repo replica a branch per repo (the point of B2)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null, // multi-repo: the single-repo shorthand stays null
      [REPO_A, REPO_B],
      ["feature/943", "main"]
    );

    expect(badgesFor([REPO_A, REPO_B]).map(formatReplicaRepoBadgeLabel)).toEqual([
      "AgentsCommander/feature/943",
      "webpage/main",
    ]);
  });

  it("follows the PATH, not the position, when config.json reorders the repos", () => {
    // The event was emitted for [A, B]. Discovery then reports [B, A] because the
    // user reordered `repos` in config.json. The stored payload is up to 15s old.
    // An index-keyed merge would hand B the branch of A - silently, and Browse
    // Branch would open a branch that does not belong to that repo.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["branch-a", "branch-b"]
    );

    expect(badgesFor([REPO_B, REPO_A])).toEqual([
      // #1028: `dirty` is null here because badgesFor() passes no dirty map - this
      // test is about branch/path transposition and says nothing about dirty.
      { label: "webpage", sourcePath: REPO_B, branch: "branch-b", dirty: null },
      { label: "AgentsCommander", sourcePath: REPO_A, branch: "branch-a", dirty: null },
    ]);
  });

  it("yields NO branch for a repo the payload never mentioned, not a neighbour's", () => {
    // Event covered [A, B]; the user then swapped B for C in config.json.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["branch-a", "branch-b"]
    );

    expect(badgesFor([REPO_A, REPO_C]).map((badge) => badge.branch)).toEqual([
      "branch-a",
      null, // NOT "branch-b"
    ]);
  });

  it("masks the discovery shorthand with an explicit null (the branch went away)", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(REPLICA, null, [REPO_A], [null]);

    expect(badgesFor([REPO_A], "stale-from-discovery").map((badge) => badge.branch)).toEqual([
      null,
    ]);
  });

  it("survives a project reload (H1): the badges keep their per-repo branches", () => {
    // projectStore.reloadProject() -> clearForPaths() on every loop tick, CLI
    // refresh, entity creation... Before the fix this wiped the map, the badges
    // fell back to the (null) single-repo shorthand, and the backend never
    // re-emitted, so Browse Branch was gone until the app restarted.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["feature/943", "main"]
    );

    replicaVolatileStore.clearForPaths([REPLICA]);

    expect(badgesFor([REPO_A, REPO_B]).map(formatReplicaRepoBadgeLabel)).toEqual([
      "AgentsCommander/feature/943",
      "webpage/main",
    ]);
  });

  it("keeps the pre-B2 fallback until the first branch event lands", () => {
    // Single repo: the discovery shorthand still drives the badge.
    expect(badgesFor([REPO_A], "feature/from-discovery").map((badge) => badge.branch)).toEqual([
      "feature/from-discovery",
    ]);
    // Multi-repo: no shorthand exists, so no branch - the 15s window the user
    // accepted, and the pre-B2 behavior.
    expect(
      badgesFor([REPO_A, REPO_B], "feature/from-discovery").map((badge) => badge.branch)
    ).toEqual([null, null]);
  });
});

// #1028 - worktree-dirty rides the B2 feed and is merged by path for the same
// reason. The stake is different though: a transposed BRANCH opens the wrong
// Browse Branch page, a transposed DIRTY accuses a clean repo of holding
// uncommitted work (or, worse, clears the accusation from one that does).
describe("#1028 per-repo dirty is merged BY PATH", () => {
  const REPLICA = "C:\\proj\\.ac\\wg-1-team\\__agent_coord";
  const REPO_A = "C:\\proj\\.ac\\wg-1-team\\repo-AgentsCommander";
  const REPO_B = "C:\\proj\\.ac\\wg-1-team\\repo-webpage";
  const REPO_C = "C:\\proj\\.ac\\wg-1-team\\repo-docs";

  beforeEach(() => {
    replicaVolatileStore.clearAll();
  });

  /** Exactly what configuredReplicaRepoBadgesLive (ProjectPanel) passes. */
  const badgesFor = (repoPaths: string[]) =>
    configuredReplicaRepoBadges(
      {
        repoPaths,
        repoBranch: undefined,
        repoBranchByPath: effectiveRepoBranchByPath({ path: REPLICA }),
        repoDirtyByPath: effectiveRepoDirtyByPath({ path: REPLICA }),
      },
      { repoPath: undefined }
    );

  it("resolves each repo's dirty from the map by path", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["main", "main"],
      [true, false]
    );

    expect(badgesFor([REPO_A, REPO_B]).map((badge) => badge.dirty)).toEqual([true, false]);
  });

  it("follows the PATH, not the position, when config.json reorders the repos", () => {
    // The event was emitted for [A, B]; discovery then reports [B, A] because the
    // user reordered `repos`. An index-keyed merge would paint A's red onto B.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["main", "main"],
      [true, false]
    );

    expect(badgesFor([REPO_B, REPO_A]).map((badge) => badge.dirty)).toEqual([false, true]);
  });

  it("yields null for a repo the payload never mentioned, not a neighbour's dirty", () => {
    // Event covered [A, B]; the user then swapped B for C in config.json.
    replicaVolatileStore.applyDiscoveryBranchUpdate(
      REPLICA,
      null,
      [REPO_A, REPO_B],
      ["main", "main"],
      [true, true]
    );

    expect(badgesFor([REPO_A, REPO_C]).map((badge) => badge.dirty)).toEqual([
      true,
      null, // NOT true
    ]);
  });

  it("yields null before any event lands: there is no single-repo shorthand for dirty", () => {
    // Unlike `branch`, which falls back to the discovery shorthand here, dirty has no
    // scalar counterpart on AcAgentReplica. This is the <=15s post-launch state.
    expect(badgesFor([REPO_A]).map((badge) => badge.dirty)).toEqual([null]);
  });

  it("keeps a detected `false` distinct from an unknown `null` all the way to the badge", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(REPLICA, null, [REPO_A], ["main"], [false]);

    const [clean, unknown] = badgesFor([REPO_A, REPO_C]);
    expect(clean.dirty).toBe(false);
    expect(unknown.dirty).toBe(null);
    // Both render violet, so the ONLY user-visible difference is the tooltip. This
    // pair is what makes a `?? null` -> `|| null` regression observable at all.
    expect(formatReplicaRepoBadgeTitle(clean)).toBe(REPO_A);
    expect(formatReplicaRepoBadgeTitle(unknown)).toBe(`${REPO_C} (status unknown)`);
  });

  it("survives a project reload (AC 6): the badges keep their dirty state", () => {
    replicaVolatileStore.applyDiscoveryBranchUpdate(REPLICA, null, [REPO_A], ["main"], [true]);

    replicaVolatileStore.clearForPaths([REPLICA]);

    expect(badgesFor([REPO_A]).map((badge) => badge.dirty)).toEqual([true]);
  });

  it("leaves dirty null for callers that do not read the volatile layer", () => {
    // `replicaRepoMenuEntries` / `replicaSearchText` pass no dirty map: they reach
    // only label/branch, and must keep compiling and behaving unchanged.
    replicaVolatileStore.applyDiscoveryBranchUpdate(REPLICA, null, [REPO_A], ["main"], [true]);

    const badges = configuredReplicaRepoBadges(
      { repoPaths: [REPO_A], repoBranch: "main" },
      { repoPath: undefined }
    );
    expect(badges.map((badge) => badge.dirty)).toEqual([null]);
    expect(badges.map(formatReplicaRepoBadgeLabel)).toEqual(["AgentsCommander/main"]);
  });
});

// #1028 - the tooltip is the only surface that distinguishes all three states, so
// the colour can stay a binary alarm at 8px.
describe("#1028 badge title carries the third state", () => {
  const REPO = "C:\\proj\\.ac\\wg-1-team\\repo-AgentsCommander";

  it("names local work not confirmed by cached origin tracking when the repo is red", () => {
    expect(formatReplicaRepoBadgeTitle({ sourcePath: REPO, dirty: true })).toBe(
      `${REPO} (local work not confirmed by cached origin tracking)`
    );
  });

  it("leaves the title exactly as it was when the repo is known clean", () => {
    // The pre-#1028 title, unchanged: a clean repo must not gain tooltip noise.
    expect(formatReplicaRepoBadgeTitle({ sourcePath: REPO, dirty: false })).toBe(REPO);
  });

  it("says status unknown - the normal state for the first <=15s after launch", () => {
    // Deliberately worded as "not yet known", not as a fault: it is the first thing
    // a user sees on every launch, before the first watcher tick lands.
    expect(formatReplicaRepoBadgeTitle({ sourcePath: REPO, dirty: null })).toBe(
      `${REPO} (status unknown)`
    );
  });
});
