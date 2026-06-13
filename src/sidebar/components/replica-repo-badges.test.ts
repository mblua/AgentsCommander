import { describe, expect, it } from "vitest";
import {
  automationIdPart,
  configuredReplicaRepoBadges,
  formatReplicaRepoBadgeLabel,
  repoLabelFromPath,
} from "./replica-repo-badges";

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
