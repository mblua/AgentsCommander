import type {
  AcAgentReplica,
  AcWorkgroup,
  RepoBranchByPath,
  SessionRepo,
} from "../../shared/types";

export function stripRepoPrefix(name: string): string {
  return name.startsWith("repo-") ? name.slice(5) : name;
}

export function repoLabelFromPath(path: string): string {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  const dirName = normalized.split("/").pop() ?? "";
  return stripRepoPrefix(dirName);
}

export function formatReplicaRepoBadgeLabel(repo: Pick<SessionRepo, "label" | "branch">): string {
  return `${repo.label}${repo.branch ? `/${repo.branch}` : ""}`;
}

export function configuredReplicaRepoBadges(
  replica: Pick<AcAgentReplica, "repoPaths" | "repoBranch"> & {
    /** #943 B2 - live per-repo branches. Optional: absent before the first branch
     *  event, and absent for callers that do not read the volatile layer. */
    repoBranchByPath?: RepoBranchByPath;
  },
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  const repoPaths = replica.repoPaths ?? [];
  const sourcePaths = repoPaths.length > 0
    ? repoPaths
    : workgroup.repoPath
      ? [workgroup.repoPath]
      : [];
  // Pre-B2 shorthand: discovery detects a branch only for a SINGLE-repo replica
  // (ac_discovery.rs `repo_paths.len() == 1`), so a multi-repo replica had none.
  // It stays as the fallback for any repo the per-repo layer has not covered: the
  // window between project load and the first watcher tick (<=15s), the legacy
  // workgroup.repoPath source above, or a repo added to config.json since the last
  // tick. NOT after a reload: `clearForPaths` deliberately preserves the per-repo
  // map, because discovery has no counterpart to re-supply it (H1).
  const singleRepoBranch = sourcePaths.length === 1 ? replica.repoBranch ?? null : null;
  const byPath = replica.repoBranchByPath;

  return sourcePaths
    .map((sourcePath) => {
      // #943 B2 - keyed BY PATH, never by position. The branch payload is stored
      // and re-read against a LATER discovery snapshot, so if config.json `repos`
      // is reordered or an entry removed, a positional merge would paint repo A's
      // branch onto repo B and Browse Branch would open a branch that does not
      // belong to that repo. A path miss yields no branch instead.
      // `undefined` = unknown path (fall back); `null` = known, no branch (mask).
      const live = byPath?.[sourcePath];
      return {
        label: repoLabelFromPath(sourcePath),
        sourcePath,
        branch: live === undefined ? singleRepoBranch : live,
      };
    })
    .filter((repo) => repo.label.length > 0);
}

export function automationIdPart(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}
