import type { AcAgentReplica, AcWorkgroup, SessionRepo } from "../../shared/types";

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
  replica: Pick<AcAgentReplica, "repoPaths" | "repoBranch">,
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  const repoPaths = replica.repoPaths ?? [];
  const sourcePaths = repoPaths.length > 0
    ? repoPaths
    : workgroup.repoPath
      ? [workgroup.repoPath]
      : [];
  const branch = sourcePaths.length === 1 ? replica.repoBranch ?? null : null;

  return sourcePaths
    .map((sourcePath) => ({
      label: repoLabelFromPath(sourcePath),
      sourcePath,
      branch,
    }))
    .filter((repo) => repo.label.length > 0);
}

export function automationIdPart(value: string): string {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}
