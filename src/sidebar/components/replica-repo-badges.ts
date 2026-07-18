import type {
  AcAgentReplica,
  AcWorkgroup,
  RepoBranchByPath,
  RepoDirtyByPath,
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

export function formatReplicaRepoBadgeTitle(repo: Pick<SessionRepo, "sourcePath" | "dirty">): string {
  if (repo.dirty === true) return `${repo.sourcePath} (uncommitted changes)`;
  if (repo.dirty === false) return repo.sourcePath;
  return `${repo.sourcePath} (status unknown)`;
}

export function configuredReplicaRepoBadges(
  replica: Pick<AcAgentReplica, "repoPaths" | "repoBranch"> & {
    repoBranchByPath?: RepoBranchByPath;
    repoDirtyByPath?: RepoDirtyByPath;
  },
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  const repoPaths = replica.repoPaths ?? [];
  const sourcePaths = repoPaths.length > 0
    ? repoPaths
    : workgroup.repoPath
      ? [workgroup.repoPath]
      : [];
  const singleRepoBranch = sourcePaths.length === 1 ? replica.repoBranch ?? null : null;
  const byPath = replica.repoBranchByPath;
  const dirtyByPath = replica.repoDirtyByPath;

  return sourcePaths
    .map((sourcePath) => {
      const live = byPath?.[sourcePath];
      return {
        label: repoLabelFromPath(sourcePath),
        sourcePath,
        branch: live === undefined ? singleRepoBranch : live,
        dirty: dirtyByPath?.[sourcePath] ?? null,
      };
    })
    .filter((repo) => repo.label.length > 0);
}

export function automationIdPart(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}
