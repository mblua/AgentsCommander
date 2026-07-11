import { beforeEach, describe, expect, it } from "vitest";
import {
  automationIdPart,
  configuredReplicaRepoBadges,
  formatReplicaRepoBadgeLabel,
  repoLabelFromPath,
} from "./replica-repo-badges";
import {
  effectiveRepoBranchByPath,
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
      { label: "webpage", sourcePath: REPO_B, branch: "branch-b" },
      { label: "AgentsCommander", sourcePath: REPO_A, branch: "branch-a" },
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
